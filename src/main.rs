use axum::debug_handler;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use futures::{SinkExt, StreamExt};
use listenfd::ListenFd;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:9001";

struct AppState {
    clients_map: Mutex<HashMap<String, String>>,
    tx: broadcast::Sender<String>,
}

#[derive(Deserialize, Debug)]
struct Input {
    token: String,
}

#[derive(Serialize, Debug)]
struct LoginResult {
    code: &'static str,
    message: &'static str,
    token: String,
}

#[derive(Serialize, Debug)]
struct CardData {
    #[serde(rename = "cardNum")]
    card_num: String,
    mtype: String,
    aids: String,
}
#[derive(Deserialize, Serialize, Debug)]
struct BaseMessage {
    cmd: String,
    #[serde(rename = "type")]
    mtype: String,
    token: String,
    data: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(move |_| "nfc-service=trace".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let clients_map = Mutex::new(HashMap::new());
    let (tx, _rx) = broadcast::channel(100);

    let app_state = Arc::new(AppState { clients_map: clients_map, tx });

    let app = Router::new()
        .route("/", get(index))
        .route("/login/doLogin", post(do_login))
        .route("/login/getInfo", post(get_info))
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
    while let Some(Ok(message)) = receiver.next().await {
        if let Message::Text(content) = message {
            if let Ok(msg) = serde_json::from_str::<BaseMessage>(&content) {
                let token = &msg.token;
                if msg.cmd.eq("init") {
                    info!("device {} has been connected", token);
                    break;
                }
            }
        }
    }
    let mut rx = state.tx.subscribe();
    let mut send_task = tokio::spawn(async move {
        while let Ok(content) = rx.recv().await {
            info!("receive msg: {}", content);
            if let Ok(msg) = serde_json::from_str::<BaseMessage>(&content) {
                _ = sender.send(Message::Text(content)).await;
            }
        }
    });

    let tx = state.tx.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(content))) = receiver.next().await {
            // if let Ok(msg) = serde_json::from_str::<BaseMessage>(&content) {
            //     if msg.token.eq(&token) && msg.cmd.eq("scan_info") {
            //         info!("sending message: {}", &content);
            //         _ = tx.send(content);
            //     }
            // }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    state.clients_map.lock().unwrap().remove(&current_token);
}

async fn index() -> Html<&'static str> {
    Html("service is on")
}

#[derive(Serialize, Debug)]
struct LoginForm {
    username: String,
    pwd: String,
    #[serde(rename = "type")]
    mtype: String,
    device: String,
}

async fn do_login(Form(login_data): Form<LoginForm>, State(state): State<Arc<AppState>>) -> (StatusCode, Json<LoginResult>) {
    let mut clients_map = state.clients_map.lock().unwrap();
    let mut client_id = String::new();
    client_id.push_str(&login_data.username);
    client_id.push_str(&login_data.pwd);
    if clients_map.contains_key(&client_id) {
        let token = clients_map.get(&client_id).unwrap_or(&Uuid::new_v4().to_string());
        (StatusCode::OK,
         Json::from(LoginResult {
             code: "1",
             message: "ok",
             token: token.into_string(),
         }))
    } else {
        let uuid = Uuid::new_v4();
        clients_map.insert(client_id, uuid.to_string());
        (StatusCode::OK,
         Json::from(LoginResult {
             code: "1",
             message: "ok",
             token: uuid.to_string(),
         }))
    }
}

#[debug_handler]
async fn get_info(Form(input): Form<Input>) -> (StatusCode, Json<LoginResult>) {
    (StatusCode::OK,
     Json::from(LoginResult {
         code: "1",
         message: "登录成功",
         token: input.token,
     }))
}