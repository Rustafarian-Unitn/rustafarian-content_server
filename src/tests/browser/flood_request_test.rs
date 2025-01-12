#[cfg(test)]
#[allow(unused)]
pub mod flood_request_test {
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
    fn flood_request_test() {
        let (mut server, neighbor, _, _) = build_server();

        let flood_request = FloodRequest {
            flood_id: 1,
            initiator_id: 21,
            path_trace: vec![(4, NodeType::Drone), (3, NodeType::Drone)],
        };
        let file_request =
            Packet::new_flood_request(SourceRoutingHeader::empty_route(), 3, flood_request);

        server.handle_drone_packets(Ok(file_request));

        let received_packet = neighbor.1.recv().unwrap();

        let flood_request_2 = FloodRequest {
            flood_id: 1,
            initiator_id: 21,
            path_trace: vec![
                (4, NodeType::Drone),
                (3, NodeType::Drone),
                (1, NodeType::Server),
            ],
        };

        let expected_packet =
            Packet::new_flood_request(SourceRoutingHeader::empty_route(), 3, flood_request_2);

        assert_eq!(expected_packet, received_packet, "Do not correspond");
    }
}
