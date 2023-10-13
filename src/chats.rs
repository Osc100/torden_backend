use crate::{
    structs::{AppState, ChannelState, GPTMessage, MessageRole},
    utils::{decode_jwt, query_to_openai},
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
use std::{net::SocketAddr, str::FromStr, sync::Arc};
use tokio::sync::{broadcast, Mutex};

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
) -> () {
    let (mut sender, mut receiver) = stream.split();
    let mut transmitters: Vec<(Uuid, broadcast::Sender<GPTMessage>)> = vec![];

    tracing::debug!("{} connected", who);

    // Manage what to do on the first message
    let first_message_status =
        handle_first_message(&mut receiver, &mut sender, &pool, who, &state).await;

    match first_message_status {
        FirstMessageResult::Break => return,
        FirstMessageResult::ContinueSingleChannel(result) => {
            transmitters.push(result);
        }
        FirstMessageResult::ContinueMultipleChannels(channels) => {
            transmitters = channels;
        }
    }

    let sender_arc = Arc::new(Mutex::new(sender));
    let receiver_arc = Arc::new(Mutex::new(receiver));

    for (channel, transmitter) in transmitters {
        let mut rx = transmitter.subscribe();
        // Send messages from other clients to this client.
        let state = state.clone();
        let pool = pool.clone();
        let sender_cloned_arc = sender_arc.clone();
        let receiver_cloned_arc = receiver_arc.clone();

        let mut send_task = {
            tokio::spawn({
                async move {
                    while let Ok(msg) = rx.recv().await {
                        // In any websocket error, break loop.

                        if sender_cloned_arc
                            .lock()
                            .await
                            .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                            .await
                            .is_err()
                        {
                            break;
                        }
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
                let mut receiver = receiver_cloned_arc.lock().await;

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
}

pub enum FirstMessageResult {
    ContinueSingleChannel((Uuid, broadcast::Sender<GPTMessage>)),
    ContinueMultipleChannels(Vec<(Uuid, broadcast::Sender<GPTMessage>)>),
    Break,
}

pub async fn handle_first_message(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    pool: &Pool<Postgres>,
    who: SocketAddr,
    state: &Arc<AppState>,
) -> FirstMessageResult {
    let channel: Uuid;

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
                return FirstMessageResult::Break;
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
                        return FirstMessageResult::Break;
                    };

                    tracing::debug!("{} created new UUID {}", who, result.id);
                    channel = result.id;
                }
                FirstMessageType::ExistingUUID => {
                    let Ok(existing_uuid) = Uuid::from_str(&parsed_message.message_content) else {
                        sender
                            .send(Message::Text("Invalid UUID".to_string()))
                            .await
                            .unwrap();
                        return FirstMessageResult::Break;
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
                        return FirstMessageResult::Break;
                    };

                    channel = query_result.id;
                }
                FirstMessageType::ChatAgent => {
                    let Ok(token_data) = decode_jwt(&parsed_message.message_content) else {
                        sender
                            .send(Message::Text("Invalid token".to_string()))
                            .await
                            .unwrap();
                        return FirstMessageResult::Break;
                    };

                    //Add agent to agent_pool in app state
                    {
                        let mut agent_pool = state.agent_pool.lock().await;
                        let agents = agent_pool
                            .entry(token_data.claims.company_id)
                            .or_insert(vec![]);

                        agents.push(token_data.claims);
                    }

                    // Get all channels for this company.
                    // TODO: FILTER BY COMPANY ID
                    return FirstMessageResult::ContinueMultipleChannels(
                        state
                            .channels
                            .lock()
                            .await
                            .iter()
                            .map(|c| (c.0.clone(), c.1.transmitter.clone()))
                            .collect(),
                    );
                }
            }

            {
                let mut channels = state.channels.lock().await;

                let channel_state = channels
                    .entry(channel.clone())
                    .or_insert(ChannelState::from_db(pool.clone(), channel.clone()).await);

                let transmitter = Some(channel_state.transmitter.clone()).unwrap();
                let messages = channel_state.messages.clone();
                // Send all existing messages to the client.
                sender
                    .send(Message::Text(serde_json::to_string(&messages).unwrap()))
                    .await
                    .unwrap();

                return FirstMessageResult::ContinueSingleChannel((channel, transmitter));
            }
        }
    }

    return FirstMessageResult::Break;
}
