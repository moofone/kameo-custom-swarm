use std::{borrow::Cow, env::args, time::Duration};

use kameo::{
    actor::ActorID,
    error::RemoteSendError,
    remote::{
        ActorSwarm, ActorSwarmBehaviourEvent, ActorSwarmEvent, ActorSwarmHandler, SwarmBehaviour,
        SwarmRequest, SwarmResponse,
    },
};
use libp2p_core::{Multiaddr, PeerId};
use libp2p_request_response::{OutboundRequestId, ProtocolSupport, ResponseChannel};
use libp2p_swarm::{Swarm, SwarmEvent};
//
// use libp2p::{
//     kad::{
//         self,
//         store::{MemoryStore, RecordStore},
//     },
//     mdns,
//     request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel},
//     swarm::{NetworkBehaviour, SwarmEvent},
//     Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder,
// };
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = args()
        .nth(1)
        .expect("expected the listen port as an argument")
        .parse()
        .expect("invalid listen port");

    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_quic()
        .with_behaviour(|key| {
            let kademlia = kad::Behaviour::new(
                key.public().to_peer_id(),
                MemoryStore::new(key.public().to_peer_id()),
            );
            Ok(CustomBehaviour {
                kademlia,
                actor_request_response: libp2p_request_response::Behaviour::new(
                    [(StreamProtocol::new("/kameo/1"), ProtocolSupport::Full)],
                    libp2p_request_response::Config::default(),
                ),
                custom_request_response: libp2p_request_response::cbor::Behaviour::new(
                    [(StreamProtocol::new("/custom/1"), ProtocolSupport::Full)],
                    libp2p_request_response::Config::default(),
                ),
                mdns: libp2p_mdns::tokio::Behaviour::new(
                    libp2p_mdns::Config::default(),
                    key.public().to_peer_id(),
                )?,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    let (actor_swarm, mut swarm_handler) =
        ActorSwarm::bootstrap_manual(*swarm.local_peer_id()).unwrap();

    actor_swarm.listen_on(format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?);

    loop {
        tokio::select! {
            Some(cmd) = swarm_handler.next_command() => swarm_handler.handle_command(&mut swarm, cmd),
            Some(event) = swarm.next() => {
                handle_event(&mut swarm, &mut swarm_handler, event);
            }
        }
    }
}

fn handle_event(
    swarm: &mut Swarm<CustomBehaviour>,
    swarm_handler: &mut ActorSwarmHandler,
    event: SwarmEvent<CustomBehaviourEvent>,
) {
    match event {
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            let name = swarm.local_peer_id().to_string();
            swarm
                .behaviour_mut()
                .custom_request_response
                .send_request(&peer_id, CustomRequest::Greet { name });
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            connection_id,
            endpoint,
            num_established,
            cause,
        } => {
            swarm_handler.handle_event(
                swarm,
                ActorSwarmEvent::ConnectionClosed {
                    peer_id,
                    connection_id,
                    endpoint,
                    num_established,
                    cause,
                },
            );
        }
        SwarmEvent::Behaviour(event) => match event {
            CustomBehaviourEvent::Kademlia(event) => swarm_handler.handle_event(
                swarm,
                ActorSwarmEvent::Behaviour(ActorSwarmBehaviourEvent::Kademlia(event)),
            ),
            CustomBehaviourEvent::ActorRequestResponse(event) => swarm_handler.handle_event(
                swarm,
                ActorSwarmEvent::Behaviour(ActorSwarmBehaviourEvent::RequestResponse(event)),
            ),
            CustomBehaviourEvent::Mdns(event) => swarm_handler.handle_event(
                swarm,
                ActorSwarmEvent::Behaviour(ActorSwarmBehaviourEvent::Mdns(event)),
            ),
            CustomBehaviourEvent::CustomRequestResponse(libp2p_request_response::Event::Message {
                message,
                ..
            }) => match message {
                libp2p_request_response::Message::Request {
                    request, channel, ..
                } => match request {
                    CustomRequest::Greet { name } => {
                        swarm
                            .behaviour_mut()
                            .custom_request_response
                            .send_response(
                                channel,
                                CustomResponse::Greeted {
                                    msg: format!("Hello, {name}"),
                                },
                            )
                            .unwrap();
                    }
                },
                libp2p_request_response::Message::Response { response, .. } => match response {
                    CustomResponse::Greeted { msg } => {
                        println!("Greeted: {msg}");
                    }
                },
            },
            _ => {}
        },
        _ => {}
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum CustomRequest {
    Greet { name: String },
}

#[derive(Debug, Serialize, Deserialize)]
enum CustomResponse {
    Greeted { msg: String },
}

#[derive(NetworkBehaviour)]
struct CustomBehaviour {
    kademlia: libp2p_kad::Behaviour<libp2p_kad::store::MemoryStore>,
    actor_request_response: libp2p_request_response::Behaviour<SwarmRequest, SwarmResponse>,
    custom_request_response: libp2p_request_response::cbor::Behaviour<CustomRequest, CustomResponse>,
    mdns: libp2p_mdns::tokio::Behaviour,
}

impl SwarmBehaviour for CustomBehaviour {
    fn ask(
        &mut self,
        peer: &PeerId,
        actor_id: ActorID,
        actor_remote_id: Cow<'static, str>,
        message_remote_id: Cow<'static, str>,
        payload: Vec<u8>,
        mailbox_timeout: Option<Duration>,
        reply_timeout: Option<Duration>,
        immediate: bool,
    ) -> OutboundRequestId {
        self.actor_request_response.send_request(
            peer,
            SwarmRequest::Ask {
                actor_id,
                actor_remote_id,
                message_remote_id,
                payload,
                mailbox_timeout,
                reply_timeout,
                immediate,
            },
        )
    }

    fn tell(
        &mut self,
        peer: &PeerId,
        actor_id: ActorID,
        actor_remote_id: Cow<'static, str>,
        message_remote_id: Cow<'static, str>,
        payload: Vec<u8>,
        mailbox_timeout: Option<Duration>,
        immediate: bool,
    ) -> OutboundRequestId {
        self.actor_request_response.send_request(
            peer,
            SwarmRequest::Tell {
                actor_id,
                actor_remote_id,
                message_remote_id,
                payload,
                mailbox_timeout,
                immediate,
            },
        )
    }

    fn link(
        &mut self,
        actor_id: ActorID,
        actor_remote_id: Cow<'static, str>,
        sibbling_id: ActorID,
        sibbling_remote_id: Cow<'static, str>,
    ) -> OutboundRequestId {
        self.actor_request_response.send_request(
            actor_id.peer_id().unwrap(),
            SwarmRequest::Link {
                actor_id,
                actor_remote_id,
                sibbling_id,
                sibbling_remote_id,
            },
        )
    }

    fn unlink(
        &mut self,
        actor_id: ActorID,
        actor_remote_id: Cow<'static, str>,
        sibbling_id: ActorID,
    ) -> OutboundRequestId {
        self.actor_request_response.send_request(
            actor_id.peer_id().unwrap(),
            SwarmRequest::Unlink {
                actor_id,
                actor_remote_id,
                sibbling_id,
            },
        )
    }

    fn signal_link_died(
        &mut self,
        dead_actor_id: ActorID,
        notified_actor_id: ActorID,
        notified_actor_remote_id: Cow<'static, str>,
        stop_reason: kameo::error::ActorStopReason,
    ) -> OutboundRequestId {
        self.actor_request_response.send_request(
            notified_actor_id.peer_id().unwrap(),
            SwarmRequest::SignalLinkDied {
                dead_actor_id,
                notified_actor_id,
                notified_actor_remote_id,
                stop_reason,
            },
        )
    }

    fn send_ask_response(
        &mut self,
        channel: ResponseChannel<kameo::remote::SwarmResponse>,
        result: Result<Vec<u8>, RemoteSendError<Vec<u8>>>,
    ) -> Result<(), SwarmResponse> {
        self.actor_request_response
            .send_response(channel, SwarmResponse::Ask(result))
    }

    fn send_tell_response(
        &mut self,
        channel: ResponseChannel<kameo::remote::SwarmResponse>,
        result: Result<(), RemoteSendError<Vec<u8>>>,
    ) -> Result<(), SwarmResponse> {
        self.actor_request_response
            .send_response(channel, SwarmResponse::Tell(result))
    }

    fn send_link_response(
        &mut self,
        channel: ResponseChannel<SwarmResponse>,
        result: Result<(), RemoteSendError<kameo::error::Infallible>>,
    ) -> Result<(), SwarmResponse> {
        self.actor_request_response
            .send_response(channel, SwarmResponse::Link(result))
    }

    fn send_unlink_response(
        &mut self,
        channel: ResponseChannel<SwarmResponse>,
        result: Result<(), RemoteSendError<kameo::error::Infallible>>,
    ) -> Result<(), SwarmResponse> {
        self.actor_request_response
            .send_response(channel, SwarmResponse::Unlink(result))
    }

    fn send_signal_link_died_response(
        &mut self,
        channel: ResponseChannel<SwarmResponse>,
        result: Result<(), RemoteSendError<kameo::error::Infallible>>,
    ) -> Result<(), SwarmResponse> {
        self.actor_request_response
            .send_response(channel, SwarmResponse::SignalLinkDied(result))
    }

    fn kademlia_add_address(&mut self, peer: &PeerId, address: Multiaddr) -> libp2p_kad::RoutingUpdate {
        self.kademlia.add_address(peer, address)
    }

    fn kademlia_set_mode(&mut self, mode: Option<libp2p_kad::Mode>) {
        self.kademlia.set_mode(mode)
    }

    fn kademlia_get_record(&mut self, key: libp2p_kad::RecordKey) -> libp2p_kad::QueryId {
        self.kademlia.get_record(key)
    }

    fn kademlia_get_record_local(&mut self, key: &libp2p_kad::RecordKey) -> Option<Cow<'_, libp2p_kad::Record>> {
        self.kademlia.store_mut().get(key)
    }

    fn kademlia_put_record(
        &mut self,
        record: libp2p_kad::Record,
        quorum: libp2p_kad::Quorum,
    ) -> Result<libp2p_kad::QueryId, libp2p_kad::store::Error> {
        self.kademlia.put_record(record, quorum)
    }

    fn kademlia_put_record_local(&mut self, record: libp2p_kad::Record) -> Result<(), libp2p_kad::store::Error> {
        self.kademlia.store_mut().put(record)
    }
}
