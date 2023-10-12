use crate::{
    structs::{AppState, ChannelState, GPTMessage, MessageRole},
    utils::query_to_openai,
};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    response::Response,
    Extension,
};
use futures::{SinkExt, StreamExt};
use sqlx::{types::Uuid, Pool, Postgres};
use std::{net::SocketAddr, ops::ControlFlow, str::FromStr, sync::Arc};
use tokio::sync::broadcast;

use crate::structs::{FirstMessage, FirstMessageType};
pub async fn ws_handler(
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

pub async fn handle_socket(
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

pub async fn handle_first_message(
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

            // serde_json::from_str::<FirstMessage>(&first_message_string).unwrap();

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
