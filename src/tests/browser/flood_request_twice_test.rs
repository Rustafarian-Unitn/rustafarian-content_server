#[cfg(test)]
#[allow(unused)]
pub mod flood_request_twice_test {
    use rustafarian_shared::{
        assembler::{assembler::Assembler, disassembler::Disassembler},
        messages::{
            browser_messages::{
                BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper,
            },
            commander_messages::{
                SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper,
            },
            general_messages::{DroneSend, ServerType, ServerTypeResponse},
        },
    };
    use wg_2024::{
        config::Drone,
        network::SourceRoutingHeader,
        packet::{FloodRequest, Fragment, NodeType, Packet, PacketType},
    };

    use crate::tests::utils::build_server;

    #[test]
    fn flood_request_twice_test() {
        let (mut server, neighbor, _, _) = build_server();

        server.send_flood_request();
        server.send_flood_request();
    }
}
