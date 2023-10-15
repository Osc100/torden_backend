use crate::{
    structs::{AppState, ChannelState, GPTMessage, MessageRole, WSMessage},
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
use serde_json::json;
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
    let mut transmitters: Vec<(Uuid, broadcast::Sender<WSMessage>)> = vec![];

    tracing::debug!("{} connected", who);
    let role: MessageRole;

    // Manage what to do on the first message
    let first_message_status =
        handle_first_message(&mut receiver, &mut sender, &pool, &state).await;

    match first_message_status {
        FirstMessageResult::Break => return,
        FirstMessageResult::ContinueSingleChannel(result) => {
            role = MessageRole::User;
            transmitters.push(result);
        }
        FirstMessageResult::ContinueMultipleChannels(channels) => {
            role = MessageRole::Agent;
            transmitters = channels;
        }
    }

    let mut send_task_vec = vec![];

    {
        let transmitters = transmitters.clone();
        let sender_arc = Arc::new(Mutex::new(sender));

        // let state = state.clone();
        // let pool = pool.clone();

        for (channel, transmitter) in transmitters {
            let sender_clone = sender_arc.clone();

            let task = tokio::spawn({
                async move {
                    let mut rx = transmitter.subscribe();
                    // Send messages from other clients to this client.

                    while let Ok(msg) = rx.recv().await {
                        if msg.channel != channel {
                            continue;
                        }

                        // In any websocket error, break loop.
                        if sender_clone
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
            });

            send_task_vec.push(task);
        }
    };

    // Receive messages from this client and send them to other clients.
    // We need to access the `tx` variable directly again, so we can't shadow it here.
    let mut recv_task = {
        let transmitter_arc = Arc::new(transmitters);
        // Clone things we want to pass to the receiving task.
        // This task will receive messages from client and send them to broadcast subscribers.
        tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = receiver.next().await {
                let message_data = match serde_json::from_str::<WSMessage>(&text) {
                    Ok(message_data) => message_data,
                    Err(err) => {
                        tracing::error!("Error parsing message: {}", err);

                        continue;
                    }
                };

                for (channel, transmitter) in transmitter_arc.iter() {
                    // This will broadcast to any customer service agent connected to the same channel.
                    if message_data.channel != *channel {
                        continue;
                    }

                    let _message = WSMessage {
                        channel: message_data.channel,
                        message: GPTMessage {
                            role,
                            content: message_data.message.content.clone(),
                        },
                    };

                    let _ = transmitter.send(_message.clone());

                    // Save message to database.

                    // TODO: Investigate performance implications of querying while locking.
                    let mut channels = state.channels.lock().await;
                    let channel_state = channels.get_mut(&channel).unwrap();

                    channel_state.save_message(_message.clone(), &pool).await;

                    if role != MessageRole::Assistant {
                        let openai_response = query_to_openai(channel_state.messages.clone()).await;

                        let ai_message = openai_response.choices[0].message.clone();

                        let channel_ai_message = WSMessage {
                            channel: _message.channel,
                            message: ai_message,
                        };
                        let _ = transmitter.send(channel_ai_message.clone());

                        channel_state.save_message(channel_ai_message, &pool).await;
                    }
                }
            }
        })
    };

    // If the sender task fails, then disconnect all clients.
    tokio::select! {
        _ = (&mut recv_task) =>
            send_task_vec.iter_mut().for_each(|send_task| send_task.abort()),
    };
}

pub enum FirstMessageResult {
    ContinueSingleChannel((Uuid, broadcast::Sender<WSMessage>)),
    ContinueMultipleChannels(Vec<(Uuid, broadcast::Sender<WSMessage>)>),
    Break,
}

/// Decide what to do on the first message.
/// Returns a tuple with the channel UUID and the transmitter.
/// If the client is an agent, return all channels.
/// If the client is a user, return the channel UUID and the transmitter.
/// On any error it will send an error message to the client and return Break.
pub async fn handle_first_message(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    pool: &Pool<Postgres>,
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

                    channel = result.id;
                    // Sends the UUID to the client.
                    sender
                        .send(Message::Text(
                            json!({
                                "channel": channel
                            })
                            .to_string(),
                        ))
                        .await
                        .unwrap();
                }
                FirstMessageType::ExistingUUID => {
                    let existing_uuid = match Uuid::from_str(&parsed_message.message_content) {
                        Ok(uuid) => uuid,
                        Err(err) => {
                            sender.send(Message::Text(err.to_string())).await.unwrap();
                            return FirstMessageResult::Break;
                        }
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
