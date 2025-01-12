#[cfg(test)]
#[allow(unused)]
pub mod flood_response_test {
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
        packet::{FloodRequest, FloodResponse, Fragment, NodeType, Packet, PacketType},
    };

    use crate::tests::utils::build_server;

    #[test]
    fn flood_response_test_mine() {
        let (mut server, neighbor, _, _) = build_server();

        let flood_response = FloodResponse {
            flood_id: 1,
            path_trace: vec![
                (1, NodeType::Server),
                (4, NodeType::Drone),
                (3, NodeType::Drone),
            ],
        };
        let routing_header_craft = SourceRoutingHeader {
            hop_index: 2,
            hops: vec![3, 4, 1],
        };
        let file_response = Packet::new_flood_response(routing_header_craft, 3, flood_response);

        server.handle_drone_packets(Ok(file_response));

        println!("Topology {:?}", server.topology)
    }

    #[test]
    fn flood_response_test_notmine() {
        let (mut server, neighbor, _, _) = build_server();

        let flood_response = FloodResponse {
            flood_id: 1,
            path_trace: vec![
                (21, NodeType::Drone),
                (1, NodeType::Drone),
                (2, NodeType::Server),
                (4, NodeType::Drone),
                (3, NodeType::Drone),
            ],
        };
        let routing_header_craft = SourceRoutingHeader {
            hop_index: 2,
            hops: vec![3, 4, 1, 2, 21],
        };
        let file_response = Packet::new_flood_response(routing_header_craft, 3, flood_response);

        server.handle_drone_packets(Ok(file_response));
        let received_packet = neighbor.1.recv().unwrap();

        let flood_response_2 = FloodResponse {
            flood_id: 1,
            path_trace: vec![
                (21, NodeType::Drone),
                (1, NodeType::Drone),
                (2, NodeType::Server),
                (4, NodeType::Drone),
                (3, NodeType::Drone),
            ],
        };
        let routing_header_craft_2 = SourceRoutingHeader {
            hop_index: 3,
            hops: vec![3, 4, 1, 2, 21],
        };
        let file_response_2 =
            Packet::new_flood_response(routing_header_craft_2, 3, flood_response_2);

        println!("Topology {:?}", server.topology);
        assert_eq!(file_response_2, received_packet, "Do not correspond");
    }
}
