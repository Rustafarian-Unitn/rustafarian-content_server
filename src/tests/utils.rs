#[allow(unused)]
use std::collections::HashMap;

use crossbeam_channel::{unbounded, Receiver, Sender};
use rustafarian_shared::messages::{commander_messages::{
    SimControllerCommand, SimControllerResponseWrapper,
}, general_messages::ServerType};
use wg_2024::packet::Packet;

use crate::content_server::ContentServer;

pub(crate) fn build_server() -> (
    ContentServer,
    (Sender<Packet>, Receiver<Packet>),
    (Sender<SimControllerCommand>, Receiver<SimControllerCommand>),
    (
        Sender<SimControllerResponseWrapper>,
        Receiver<SimControllerResponseWrapper>,
    ),
) {
    let neighbor: (Sender<Packet>, Receiver<Packet>) = unbounded();
    let mut neighbors = HashMap::new();
    neighbors.insert(2 as u8, neighbor.0.clone());
    let channel: (Sender<Packet>, Receiver<Packet>) = unbounded();
    let server_id = 1;

    let controller_channel_commands = unbounded();
    let controller_channel_messages = unbounded();

    let mut server = ContentServer::new(
        server_id,
        neighbors,
        channel.1,
        controller_channel_commands.1.clone(),
        controller_channel_messages.0.clone(),
        "files",
        "media",
        ServerType::Text,
        false,

    );

    
    server.topology.add_node(2);
    server.topology.add_node(21);
    server.topology.add_edge(2, 21);
    server.topology.add_edge(1, 2);

    (
        server,
        neighbor,
        controller_channel_commands,
        controller_channel_messages,
    )
}
