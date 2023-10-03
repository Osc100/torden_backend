pub mod api;
pub mod structs;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    response::Response,
    routing::get,
    Extension, Json, Router,
};
use futures::{SinkExt, StreamExt};
use reqwest::Client;
use sqlx::{postgres::PgPoolOptions, types::Uuid, Pool, Postgres};
use std::{collections::HashMap, net::SocketAddr, ops::ControlFlow, str::FromStr, sync::Arc, vec};
use structs::{ChatCompletion, GPTMessage, GPTRequest, MessageRole};
use tokio::sync::{broadcast, Mutex};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::structs::{FirstMessage, FirstMessageType};
struct AppState {
    channels: Mutex<HashMap<Uuid, ChannelState>>,
}

struct ChannelState {
    messages: Vec<GPTMessage>,
    stopped_ai: bool,
    transmitter: broadcast::Sender<GPTMessage>,
}

impl ChannelState {
    async fn from_db(pool: Pool<Postgres>, uuid: Uuid) -> Self {
        let messages = sqlx::query_as!(
            GPTMessage,
            r#"SELECT role as "role: MessageRole", text as content FROM message WHERE chat_id = $1"#,
            uuid
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        Self {
            messages,
            stopped_ai: false,
            transmitter: broadcast::channel(2).0,
        }
    }

    async fn save_message(&mut self, message: GPTMessage, uuid: Uuid, pool: &Pool<Postgres>) {
        sqlx::query!(
            "INSERT INTO message (chat_id, role, text) VALUES ($1, $2, $3)",
            uuid,
            message.role as MessageRole,
            message.content
        )
        .execute(pool)
        .await
        .unwrap();

        self.messages.push(message);
    }
}

#[tokio::main]
async fn main() {
    // Load environment variables from .env file.
    dotenv::dotenv().ok();

    // Initialize tracing subscriber.
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "example_chat=trace".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Arc is a thread-safe reference-counted pointer, used to share state between threads.
    let app_state = Arc::new(AppState {
        channels: Mutex::new(HashMap::new()),
    });

    // Connection pool to the database, pased to the router as Extension.
    let pool = PgPoolOptions::new()
        .max_connections(50)
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();

    // Insert test company.
    sqlx::query!(
        "INSERT INTO company (id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        1,
        "Test Company"
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = Router::new()
        .route("/", get(ws_handler))
        .with_state(app_state)
        .layer(Extension(pool));

    let addr = SocketAddr::from(([0, 0, 0, 0], 4000));
    tracing::debug!("listening on {}", addr);

    axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    Extension(pool): Extension<Pool<Postgres>>,
) -> Response {
    let addr_string = addr.ip().to_string();
    println!("{} ", addr_string);
    println!("at {addr} connected.");

    ws.on_upgrade(move |socket| handle_socket(socket, addr, state, pool))
}

async fn handle_socket(
    stream: WebSocket,
    who: SocketAddr,
    state: Arc<AppState>,
    pool: Pool<Postgres>,
) {
    let (mut sender, mut receiver) = stream.split();
    let mut transmitter = None::<broadcast::Sender<GPTMessage>>;
    let mut channel = Uuid::nil();

    tracing::debug!("{} connected", who);

    // Manage what to do on the first message
    if let ControlFlow::Break(_) = handle_first_message(
        &mut receiver,
        &mut sender,
        &pool,
        who,
        &mut channel,
        &state,
        &mut transmitter,
    )
    .await
    {
        return;
    }

    let transmitter = transmitter.unwrap();
    let mut rx = transmitter.subscribe();
    // Send messages from other clients to this client.
    let mut send_task = {
        tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                // In any websocket error, break loop.
                if sender
                    .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
    };
    // Receive messages from this client and send them to other clients.
    // We need to access the `tx` variable directly again, so we can't shadow it here.
    let mut recv_task = {
        // Clone things we want to pass to the receiving task.
        let tx = transmitter.clone();

        // This task will receive messages from client and send them to broadcast subscribers.
        tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = receiver.next().await {
                // This will broadcast to any customer service agent connected to the same channel.

                let _message = GPTMessage {
                    role: MessageRole::User,
                    content: text.clone(),
                };
                let _ = tx.send(_message.clone());

                // Save message to database.
                let mut channels = state.channels.lock().await;
                let channel_state = channels.get_mut(&channel).unwrap();

                channel_state
                    .save_message(_message, channel.clone(), &pool)
                    .await;

                let openai_response = query_to_openai(channel_state.messages.clone()).await;

                let ai_message = openai_response.choices[0].message.clone();

                channel_state
                    .save_message(ai_message.clone(), channel, &pool)
                    .await;

                let _ = tx.send(ai_message);
            }
        })
    };

    // If either the sender or receiver task finishes, cancel the other one.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}

async fn handle_first_message(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    pool: &Pool<Postgres>,
    who: SocketAddr,
    channel: &mut Uuid,
    state: &Arc<AppState>,
    transmitter: &mut Option<broadcast::Sender<GPTMessage>>,
) -> ControlFlow<()> {
    while let Some(Ok(first_message)) = receiver.next().await {
        if let Message::Text(first_message_string) = first_message {
            println!("{} first_message ", first_message_string.clone());

            let Ok(parsed_message) = serde_json::from_str::<FirstMessage>(&first_message_string)
            else {
                sender
                    .send(Message::Text("Invalid first message".to_string()))
                    .await
                    .unwrap();
                return ControlFlow::Break(());
            };

            match parsed_message.message_type {
                FirstMessageType::NewUUID => {
                    let Ok(result) =
                        sqlx::query!("INSERT INTO chat (company_id) VALUES ($1) RETURNING id", 1)
                            .fetch_one(pool)
                            .await
                    else {
                        sender
                            .send(Message::Text(
                                "Error creating new UUID on database level.".to_string(),
                            ))
                            .await
                            .unwrap();
                        return ControlFlow::Break(());
                    };

                    tracing::debug!("{} created new UUID {}", who, result.id);
                    *channel = result.id;
                }
                FirstMessageType::ExistingUUID => {
                    let Ok(existing_uuid) = Uuid::from_str(&parsed_message.message_content) else {
                        sender
                            .send(Message::Text("Invalid UUID".to_string()))
                            .await
                            .unwrap();
                        return ControlFlow::Break(());
                    };

                    let Ok(query_result) =
                        sqlx::query!("SELECT id FROM chat WHERE id = $1", existing_uuid)
                            .fetch_one(pool)
                            .await
                    else {
                        sender
                            .send(Message::Text(
                                "The UUID doesn't exist in the database".to_string(),
                            ))
                            .await
                            .unwrap();
                        return ControlFlow::Break(());
                    };

                    *channel = query_result.id;
                }
                FirstMessageType::ChatAgent => {
                    println!("should manage chant agent")
                }
            }

            {
                let mut channels = state.channels.lock().await;

                let channel_state = channels
                    .entry(channel.clone())
                    .or_insert(ChannelState::from_db(pool.clone(), channel.clone()).await);

                *transmitter = Some(channel_state.transmitter.clone());
                let messages = channel_state.messages.clone();
                // Send all existing messages to the client.
                sender
                    .send(Message::Text(serde_json::to_string(&messages).unwrap()))
                    .await
                    .unwrap();
            }

            break;
        }
    }
    ControlFlow::Continue(())
}

async fn query_to_openai(conversation_messages: Vec<GPTMessage>) -> Json<ChatCompletion> {
    let client = Client::new();

    let mut all_messages = vec![GPTMessage {
        role: MessageRole::User,
        content: r#"
        You're a helpful sales representative who works in torden, an startup dedicated to automate customer service using LLMs. 
        Write short, helpful messages.
        Here's some more context that you will answer only if asked: 
        The founders/members are Oscar Marin as Tech Lead and Software Architect, Kelly as Marketing Specialist, Agner as Design Specialist and Katherine as relationship management and sales.
        The startup doesn't have a fisical location yet and will make it's official annoucement in the Hackathon Nicaragua 2023"#.to_string(),
    }];

    all_messages.append(&mut conversation_messages.clone());

    let request_data = GPTRequest {
        model: "gpt-3.5-turbo".to_string(),
        messages: all_messages,
        max_tokens: 100,
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&std::env::var("OPENAI_API_KEY").unwrap())
        .json(&request_data)
        .send()
        .await
        .unwrap();

    Json(response.json::<ChatCompletion>().await.unwrap())
}
