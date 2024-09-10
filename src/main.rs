use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::{get};
use axum::Router;
use futures::{SinkExt, StreamExt};
use listenfd::ListenFd;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:9001";
pub const TYPE_SERVER_CONNECT: &str = "s:connect";
pub const TYPE_SERVER_INFO: &str = "s:info";
pub const TYPE_CLIENT_CONNECT: &str = "c:connect";

struct AppState {
    clients_set: Mutex<HashSet<String>>,
    tx: broadcast::Sender<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct IMessage<'a> {
    #[serde(rename = "type")]
    mtype: &'a str,
    from: String,
    message: String,
}

#[derive(Serialize, Debug)]
struct StatusMessage<'a> {
    success: bool,
    #[serde(rename = "type")]
    mtype: &'a str,
    message: &'a str,
}

#[allow(unused)]
#[derive(Deserialize, Debug)]
struct VerificationMessage<'a> {
    #[serde(rename = "type")]
    mtype: &'a str,
    token: String,
    #[serde(rename = "groupId")]
    group_id: String,
    #[serde(rename = "groupSecret")]
    group_secret: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(move |_| "example_chat=trace".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let clients_set = Mutex::new(HashSet::new());
    let (tx, _rx) = broadcast::channel(100);

    let app_state = Arc::new(AppState { clients_set, tx });

    let app = Router::new()
        .route("/", get(index))
        .route("/websocket", get(websocket_handler))
        .with_state(app_state);

    let mut listenfd = ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0).unwrap() {
        Some(listener) => {
            listener.set_nonblocking(true).unwrap();
            TcpListener::from_std(listener).unwrap()
        }
        None => TcpListener::bind(DEFAULT_BIND_ADDR).await.unwrap(),
    };
    println!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket(socket, state))
}

async fn websocket(stream: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = stream.split();
    let mut client_name = String::new();
    while let Some(Ok(message)) = receiver.next().await {
        if let Message::Text(content) = message {
            if !content.is_empty() {
                if let Ok(msg) = serde_json::from_str::<IMessage>(&content) {
                    if msg.mtype.eq(TYPE_CLIENT_CONNECT) {
                        check_client_name(&state, &mut client_name, &msg.from);
                        if let Ok(rs) = serde_json::to_string(&StatusMessage { success: true, mtype: TYPE_SERVER_CONNECT, message: "connection establish" }) {
                            _ = sender.send(Message::Text(rs)).await;
                            break;
                        }
                    }
                } else {
                    if let Ok(rs) = serde_json::to_string(&StatusMessage { success: false, mtype: TYPE_SERVER_CONNECT, message: "connection failed" }) {
                        _ = sender.send(Message::Text(rs)).await;
                    }
                }
            }
        }
    }
    let mut rx = state.tx.subscribe();
    if let Ok(msg) = serde_json::to_string(&IMessage { from: "server".to_owned(), mtype: TYPE_SERVER_INFO, message: format!("{client_name} joined.") }) {
        tracing::debug!("{msg}");
        _ = state.tx.send(msg);
    }
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let tx = state.tx.clone();
    let name = client_name.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            tracing::debug!("{name}: {text}");
            _ = tx.send(text);
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    if let Ok(msg) = serde_json::to_string(&IMessage { from: "server".to_owned(), mtype: TYPE_SERVER_INFO, message: format!("{client_name} left.") }) {
        tracing::debug!("{msg}");
        _ = state.tx.send(msg);
    }
    state.clients_set.lock().unwrap().remove(&client_name);
}

fn check_client_name(state: &AppState, client_name: &mut String, name: &str) {
    let mut clients_set = state.clients_set.lock().unwrap();
    if !clients_set.contains(name) {
        clients_set.insert(name.to_owned());
        client_name.push_str(name);
    }
}

async fn index() -> Html<&'static str> {
    Html(include_str!("chat.html"))
}
