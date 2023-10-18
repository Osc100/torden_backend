use crate::{
    structs::{
        AgentPoolAction, AppState, ChannelState, ChannelTransmitter, Company, GPTMessage,
        MessageRole, PublicAccountData, SingleAgentAction, WSMessage,
    },
    utils::{decode_jwt, get_chats_per_agent, query_to_openai},
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
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

const AGENT_CHANNEL_BUFFER: u8 = 10;

use crate::structs::{FirstMessage, FirstMessageType};
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    Extension(pool): Extension<Pool<Postgres>>,
    Extension(agent_tx): Extension<mpsc::Sender<AgentPoolAction>>,
) -> Response {
    let addr_string = addr.ip().to_string();
    println!("{} ", addr_string);
    println!("at {addr} connected.");

    ws.on_upgrade(move |socket| handle_socket(socket, addr, state, pool, agent_tx))
}

pub async fn handle_socket(
    stream: WebSocket,
    who: SocketAddr,
    state: Arc<AppState>,
    pool: Pool<Postgres>,
    agent_tx: mpsc::Sender<AgentPoolAction>,
) -> () {
    let (mut sender, mut receiver) = stream.split();
    let mut transmitters: Vec<(Uuid, ChannelTransmitter)> = vec![];
    let mut agent_receiver: Option<mpsc::Receiver<SingleAgentAction>> = None;
    let mut agent_data: Option<PublicAccountData> = None;

    tracing::debug!("{} connected", who);
    let role: MessageRole;

    // Manage what to do on the first message
    let first_message_status =
        handle_first_message(&mut receiver, &mut sender, &pool, &state, &agent_tx).await;

    match first_message_status {
        FirstMessageResult::Break => return,
        FirstMessageResult::ContinueSingleChannel(result) => {
            role = MessageRole::User;
            transmitters.push(result);
        }
        FirstMessageResult::ContinueMultipleChannels(channels) => {
            role = MessageRole::Agent;
            transmitters = channels.channels;
            agent_data = Some(channels.account);
            agent_receiver = Some(channels.agent_receiver);
        }
    }

    let sender_arc = Arc::new(Mutex::new(sender));
    let mut send_task_vec = vec![];

    {
        let transmitters = transmitters.clone();
        // let state = state.clone();
        // let pool = pool.clone();

        for (channel, transmitter) in transmitters {
            let task = start_channel_listener(channel, transmitter, sender_arc.clone());
            send_task_vec.push(task);
        }
    };

    // Receive messages from this client and send them to other clients.
    // We need to access the `tx` variable directly again, so we can't shadow it here.
    // Clone things we want to pass to the receiving task.
    let transmitter_arc = Arc::new(transmitters);
    // Clone things we want to pass to the receiving task.
    // This task will receive messages from client and send them to broadcast subscribers.
    let mut recv_task = tokio::spawn(async move {
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
    });

    let mut agent_rx_task: Option<JoinHandle<()>> = None;
    let agent_extra_tasks = Arc::new(Mutex::new(vec![]));

    if let Some(mut agent_receiver) = agent_receiver {
        let agent_extra_tasks = agent_extra_tasks.clone();
        let tmp_task = tokio::spawn(async move {
            while let Some(action) = agent_receiver.recv().await {
                match action {
                    SingleAgentAction::AddChat((channel, transmitter)) => {
                        let task = start_channel_listener(channel, transmitter, sender_arc.clone());
                        agent_extra_tasks.lock().await.push(task);
                    }
                }
            }
        });

        agent_rx_task = Some(tmp_task);
    }
    // If the client disconnects, then...
    // Cancel all tasks.
    tokio::select! {
        _ = (&mut recv_task) =>
            {
            send_task_vec.iter_mut().for_each(|send_task| send_task.abort());
            if let Some(agent_rx_task) = agent_rx_task {
                agent_rx_task.abort();

                if let Some(account) = agent_data {
                    agent_tx.send(AgentPoolAction::RemoveAgent(account)).await.unwrap();
                }

            }
            for task in agent_extra_tasks.lock().await.iter_mut() {
                task.abort();
            }
        }
    };
}

pub struct MultipleChannelResult {
    pub channels: Vec<(Uuid, ChannelTransmitter)>,
    pub account: PublicAccountData,
    pub agent_receiver: mpsc::Receiver<SingleAgentAction>,
}
pub enum FirstMessageResult {
    ContinueSingleChannel((Uuid, ChannelTransmitter)),
    ContinueMultipleChannels(MultipleChannelResult),
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
    agent_tx: &mpsc::Sender<AgentPoolAction>,
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
                        // TODO: FILTER BY COMPANY ID
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
                    let (agent_sender, agent_receiver) =
                        mpsc::channel::<SingleAgentAction>(AGENT_CHANNEL_BUFFER.into());

                    agent_tx
                        .send(AgentPoolAction::AddAgent((
                            token_data.claims.to_owned(),
                            agent_sender,
                        )))
                        .await
                        .unwrap();

                    // Get all channels for this company.
                    // TODO: FILTER BY COMPANY ID
                    return FirstMessageResult::ContinueMultipleChannels(MultipleChannelResult {
                        account: token_data.claims.to_owned(),
                        channels: state
                            .channels
                            .lock()
                            .await
                            .iter()
                            .map(|c| (c.0.clone(), c.1.transmitter.clone()))
                            .collect(),
                        agent_receiver,
                    });
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

fn start_channel_listener(
    channel: Uuid,
    transmitter: ChannelTransmitter,
    sender: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = transmitter.subscribe();
        // Send messages from other clients to this client.

        while let Ok(msg) = rx.recv().await {
            if msg.channel != channel {
                continue;
            }

            // In any websocket error, break loop.
            if sender
                .lock()
                .await
                .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

pub fn agent_pool_task(
    app_state: Arc<AppState>,
    mut agent_receiver: mpsc::Receiver<AgentPoolAction>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(agent_action) = agent_receiver.recv().await {
            let app_state = app_state.clone();

            match agent_action {
                AgentPoolAction::AddAgent((agent, agent_sender)) => {
                    {
                        let mut agent_pool = app_state.agent_pool.lock().await;
                        let agents = agent_pool
                            .entry(Company(agent.company_id))
                            .or_insert(vec![]);

                        agents.push((agent.clone(), agent_sender.clone()));
                    }

                    {
                        assign_chats_to_agent(agent, app_state).await;
                    }
                }

                AgentPoolAction::RemoveAgent(removed_agent) => {
                    let mut agent_pool = app_state.agent_pool.lock().await;
                    let agents = agent_pool
                        .entry(Company(removed_agent.company_id))
                        .or_default();

                    for (index, agent_in_pool) in agents.iter().enumerate() {
                        if agent_in_pool.0.id == removed_agent.id {
                            agents.remove(index);
                            break;
                        }
                    }

                    {
                        // Get the agent with the least amount of chats.
                        let mut chats_per_agent =
                            get_chats_per_agent(app_state.clone(), removed_agent.company_id).await;

                        chats_per_agent.sort_unstable_by_key(|x| x.2);

                        let mut channels = app_state.channels.lock().await;

                        // Assign all chats from the removed agent to the agent with the least amount of chats.
                        if let Some((agent, agent_channel, _)) = chats_per_agent.first() {
                            for (channel, channel_state) in channels.iter_mut() {
                                if channel_state.current_agent.as_ref().is_some_and(
                                    |current_agent| current_agent.id == removed_agent.id,
                                ) {
                                    agent_channel
                                        .send(SingleAgentAction::AddChat((
                                            channel.to_owned(),
                                            channel_state.transmitter.clone(),
                                        )))
                                        .await
                                        .unwrap();
                                    channel_state.current_agent = Some(agent.to_owned());
                                }
                            }
                        }
                    }
                }

                AgentPoolAction::AssignChat(company, channel) => {
                    let mut chats_per_agent =
                        get_chats_per_agent(app_state.clone(), company.0).await;

                    chats_per_agent.sort_unstable_by_key(|x| x.2);

                    let mut channels = app_state.channels.lock().await;

                    if let Some((agent, agent_channel, _)) = chats_per_agent.first() {
                        let channel_state = channels.get_mut(&channel).unwrap();
                        agent_channel
                            .send(SingleAgentAction::AddChat((
                                channel.to_owned(),
                                channel_state.transmitter.clone(),
                            )))
                            .await
                            .unwrap();

                        channel_state.current_agent = Some(agent.to_owned());
                    }
                }
            }
        }
    })
}

async fn assign_chats_to_agent(agent: PublicAccountData, app_state: Arc<AppState>) {
    let mut channels = app_state.channels.lock().await;
    let channels_vec = channels
        .iter_mut()
        .filter(|x| x.1.company_id == agent.company_id && x.1.current_agent.is_none())
        .collect::<Vec<_>>();

    for (channel, channel_state) in channels_vec {
        channel_state.current_agent = Some(agent.clone());

        channel_state
            .transmitter
            .send(WSMessage {
                channel: channel.to_owned(),
                message: GPTMessage {
                    role: MessageRole::Status,
                    content: format!(
                        "El agente {} {} ha sido asignado al chat.",
                        agent.first_name, agent.last_name
                    ),
                },
            })
            .unwrap();
    }
}
