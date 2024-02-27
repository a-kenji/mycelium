use std::{
    net::{IpAddr, SocketAddr},
    ops::Deref,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use log::{debug, error};
use serde::{Deserialize, Serialize};

use crate::{
    crypto::PublicKey,
    message::{MessageId, MessageInfo, MessageStack},
    peer_manager::{PeerManager, PeerStats},
};

/// Default amount of time to try and send a message if it is not explicitly specified.
const DEFAULT_MESSAGE_TRY_DURATION: Duration = Duration::from_secs(60 * 5);

/// Http API server handle. The server is spawned in a background task. If this handle is dropped,
/// the server is terminated.
pub struct Http {
    /// Channel to send cancellation to the http api server. We just keep a reference to it since
    /// dropping it will also cancel the receiver and thus the server.
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone)]
/// Shared state accessible in HTTP endpoint handlers.
struct HttpServerState {
    /// Access to the (`Router`)(crate::router::Router) state. This is only meant as read only view.
    router: Arc<Mutex<crate::router::Router>>,
    /// Access to the connection state of (`Peer`)[crate::peer::Peer]s.
    peer_manager: PeerManager,
    /// Access to messages.
    message_stack: MessageStack,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSendInfo {
    pub dst: MessageDestination,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "base64::optional_binary")]
    pub topic: Option<Vec<u8>>,
    #[serde(with = "base64::binary")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageDestination {
    Ip(IpAddr),
    Pk(PublicKey),
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceiveInfo {
    pub id: MessageId,
    pub src_ip: IpAddr,
    pub src_pk: PublicKey,
    pub dst_ip: IpAddr,
    pub dst_pk: PublicKey,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "base64::optional_binary")]
    pub topic: Option<Vec<u8>>,
    #[serde(with = "base64::binary")]
    pub payload: Vec<u8>,
}

impl MessageDestination {
    /// Get the IP address of the destination.
    fn ip(self) -> IpAddr {
        match self {
            MessageDestination::Ip(ip) => ip,
            MessageDestination::Pk(pk) => IpAddr::V6(pk.address()),
        }
    }
}

impl Http {
    /// Spawns a new HTTP API server on the provided listening address.
    pub fn spawn(
        router: crate::router::Router,
        peer_manager: PeerManager,
        message_stack: MessageStack,
        listen_addr: &SocketAddr,
    ) -> Self {
        let server_state = HttpServerState {
            router: Arc::new(Mutex::new(router)),
            peer_manager,
            message_stack,
        };
        let admin_routes = Router::new()
            .route("/admin", get(get_info))
            .route("/admin/peers", get(get_peers))
            .route("/admin/routes/selected", get(get_selected_routes))
            .route("/admin/routes/fallback", get(get_fallback_routes))
            .with_state(server_state.clone());
        let msg_routes = Router::new()
            .route("/messages", get(get_message).post(push_message))
            .route("/messages/status/:id", get(message_status))
            .route("/messages/reply/:id", post(reply_message))
            .with_state(server_state);
        let app = Router::new()
            .nest("/api/v1", msg_routes)
            .nest("/api/v1", admin_routes);
        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let server = axum::Server::bind(listen_addr)
            .serve(app.into_make_service())
            .with_graceful_shutdown(async {
                cancel_rx.await.ok();
            });

        tokio::spawn(async {
            if let Err(e) = server.await {
                error!("Http API server error: {e}");
            }
        });
        Http { _cancel_tx }
    }
}

#[derive(Deserialize)]
struct GetMessageQuery {
    peek: Option<bool>,
    timeout: Option<u64>,
    /// Optional filter for start of the message, base64 encoded.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "base64::optional_binary")]
    topic: Option<Vec<u8>>,
}

impl GetMessageQuery {
    /// Did the query indicate we should peek the message instead of pop?
    fn peek(&self) -> bool {
        matches!(self.peek, Some(true))
    }

    /// Amount of seconds to hold and try and get values.
    fn timeout_secs(&self) -> u64 {
        self.timeout.unwrap_or(0)
    }
}

async fn get_message(
    State(state): State<HttpServerState>,
    Query(query): Query<GetMessageQuery>,
) -> Result<Json<MessageReceiveInfo>, StatusCode> {
    debug!(
        "Attempt to get message, peek {}, timeout {} seconds",
        query.peek(),
        query.timeout_secs()
    );

    // A timeout of 0 seconds essentially means get a message if there is one, and return
    // immediatly if there isn't. This is the result of the implementation of Timeout, which does a
    // poll of the internal future first, before polling the delay.
    tokio::time::timeout(
        Duration::from_secs(query.timeout_secs()),
        state.message_stack.message(!query.peek(), query.topic),
    )
    .await
    .or(Err(StatusCode::NO_CONTENT))
    .map(|m| {
        Json(MessageReceiveInfo {
            id: m.id,
            src_ip: m.src_ip,
            src_pk: m.src_pk,
            dst_ip: m.dst_ip,
            dst_pk: m.dst_pk,
            topic: if m.topic.is_empty() {
                None
            } else {
                Some(m.topic)
            },
            payload: m.data,
        })
    })
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageIdReply {
    id: MessageId,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum PushMessageResponse {
    Reply(MessageReceiveInfo),
    Id(MessageIdReply),
}

#[derive(Deserialize)]
struct PushMessageQuery {
    reply_timeout: Option<u64>,
}

impl PushMessageQuery {
    /// The user requested to wait for the reply or not.
    fn await_reply(&self) -> bool {
        self.reply_timeout.is_some()
    }

    /// Amount of seconds to wait for the reply.
    fn timeout(&self) -> u64 {
        self.reply_timeout.unwrap_or(0)
    }
}

async fn push_message(
    State(state): State<HttpServerState>,
    Query(query): Query<PushMessageQuery>,
    Json(message_info): Json<MessageSendInfo>,
) -> Result<(StatusCode, Json<PushMessageResponse>), StatusCode> {
    let dst = message_info.dst.ip();
    debug!(
        "Pushing new message of {} bytes to message stack for target {dst}",
        message_info.payload.len(),
    );

    let (id, sub) = match state.message_stack.new_message(
        dst,
        message_info.payload,
        if let Some(topic) = message_info.topic {
            topic
        } else {
            vec![]
        },
        DEFAULT_MESSAGE_TRY_DURATION,
        query.await_reply(),
    ) {
        Ok((id, sub)) => (id, sub),
        Err(_) => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    if !query.await_reply() {
        // If we don't wait for the reply just return here.
        return Ok((
            StatusCode::CREATED,
            Json(PushMessageResponse::Id(MessageIdReply { id })),
        ));
    }

    let mut sub = sub.unwrap();
    tokio::select! {
        sub_res = sub.changed() => {
            match sub_res {
                Ok(_) => {
                    if let Some(m) = sub.borrow().deref()  {
                        Ok((StatusCode::OK, Json(PushMessageResponse::Reply(MessageReceiveInfo {
                            id: m.id,
                            src_ip: m.src_ip,
                            src_pk: m.src_pk,
                            dst_ip: m.dst_ip,
                            dst_pk: m.dst_pk,
                            topic: if m.topic.is_empty() { None } else { Some(m.topic.clone()) },
                            payload: m.data.clone(),
                        }))))
                    } else {
                        // This happens if a none value is send, which should not happen.
                        Err(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
                Err(_)  => {
                    // This happens if the sender drops, which should not happen.
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        },
        _ = tokio::time::sleep(Duration::from_secs(query.timeout())) => {
            // Timeout expired while waiting for reply
            Ok((StatusCode::REQUEST_TIMEOUT, Json(PushMessageResponse::Id(MessageIdReply { id  }))))
        }
    }
}

async fn reply_message(
    State(state): State<HttpServerState>,
    Path(id): Path<MessageId>,
    Json(message_info): Json<MessageSendInfo>,
) -> StatusCode {
    let dst = message_info.dst.ip();
    debug!(
        "Pushing new reply to {} of {} bytes to message stack for target {dst}",
        id.as_hex(),
        message_info.payload.len(),
    );

    state
        .message_stack
        .reply_message(id, dst, message_info.payload, DEFAULT_MESSAGE_TRY_DURATION);

    StatusCode::NO_CONTENT
}

async fn message_status(
    State(state): State<HttpServerState>,
    Path(id): Path<MessageId>,
) -> Result<Json<MessageInfo>, StatusCode> {
    debug!("Fetching message status for message {}", id.as_hex());

    state
        .message_stack
        .message_info(id)
        .ok_or(StatusCode::NOT_FOUND)
        .map(Json)
}

/// Get the stats of the current known peers
async fn get_peers(State(state): State<HttpServerState>) -> Json<Vec<PeerStats>> {
    debug!("Fetching peer stats");
    Json(state.peer_manager.peers())
}

/// Alias to a [`Metric`](crate::metric::Metric) for serialization in the API.
pub enum Metric {
    /// Finite metric
    Value(u16),
    /// Infinite metric
    Infinite,
}

/// Info about a route. This uses base types only to avoid having to introduce too many Serialize
/// bounds in the core types.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// We convert the [`subnet`](Subnet) to a string to avoid introducing a bound on the actual
    /// type.
    pub subnet: String,
    /// Next hop of the route, in the underlay.
    pub next_hop: String,
    /// Computed metric of the route.
    pub metric: Metric,
    /// Sequence number of the route.
    pub seqno: u16,
}

/// List all currently selected routes.
async fn get_selected_routes(State(state): State<HttpServerState>) -> Json<Vec<Route>> {
    debug!("Loading selected routes");
    let routes = state
        .router
        .lock()
        .unwrap()
        .load_selected_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// List all active fallback routes.
async fn get_fallback_routes(State(state): State<HttpServerState>) -> Json<Vec<Route>> {
    debug!("Loading fallback routes");
    let routes = state
        .router
        .lock()
        .unwrap()
        .load_fallback_routes()
        .into_iter()
        .map(|sr| Route {
            subnet: sr.source().subnet().to_string(),
            next_hop: sr.neighbour().connection_identifier().clone(),
            metric: if sr.metric().is_infinite() {
                Metric::Infinite
            } else {
                Metric::Value(sr.metric().into())
            },
            seqno: sr.seqno().into(),
        })
        .collect();

    Json(routes)
}

/// General info about a node.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// The overlay subnet in use by the node.
    pub node_subnet: String,
}

/// Get general info about the node.
async fn get_info(State(state): State<HttpServerState>) -> Json<Info> {
    Json(Info {
        node_subnet: state.router.lock().unwrap().node_tun_subnet().to_string(),
    })
}

impl Serialize for Metric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Infinite => serializer.serialize_str("infinite"),
            Self::Value(v) => serializer.serialize_u16(*v),
        }
    }
}

/// Module to implement base64 decoding and encoding
// Sourced from https://users.rust-lang.org/t/serialize-a-vec-u8-to-json-as-base64/57781, with some
// addaptions to work with the new version of the base64 crate
mod base64 {
    use base64::alphabet;
    use base64::engine::{GeneralPurpose, GeneralPurposeConfig};

    const B64ENGINE: GeneralPurpose = base64::engine::general_purpose::GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new(),
    );

    pub mod binary {
        use super::B64ENGINE;
        use base64::Engine;
        use serde::{Deserialize, Serialize};
        use serde::{Deserializer, Serializer};

        pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
            let base64 = B64ENGINE.encode(v);
            String::serialize(&base64, s)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
            let base64 = String::deserialize(d)?;
            B64ENGINE
                .decode(base64.as_bytes())
                .map_err(serde::de::Error::custom)
        }
    }

    pub mod optional_binary {
        use super::B64ENGINE;
        use base64::Engine;
        use serde::{Deserialize, Serialize};
        use serde::{Deserializer, Serializer};

        pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            if let Some(v) = v {
                let base64 = B64ENGINE.encode(v);
                String::serialize(&base64, s)
            } else {
                <Option<String>>::serialize(&None, s)
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            if let Some(base64) = <Option<String>>::deserialize(d)? {
                B64ENGINE
                    .decode(base64.as_bytes())
                    .map_err(serde::de::Error::custom)
                    .map(Option::Some)
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finite_metric_serialization() {
        let metric = super::Metric::Value(10);
        let s = serde_json::to_string(&metric).expect("can encode finite metric");

        assert_eq!("10", s);
    }

    #[test]
    fn infinite_metric_serialization() {
        let metric = super::Metric::Infinite;
        let s = serde_json::to_string(&metric).expect("can encode infinite metric");

        assert_eq!("\"infinite\"", s);
    }
}
