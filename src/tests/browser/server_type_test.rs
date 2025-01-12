#[cfg(test)]
pub mod file_request_test {
    use rustafarian_shared::{
        assembler::{assembler::Assembler, disassembler::Disassembler},
        messages::{
            browser_messages::{
                BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper,
            },
            commander_messages::{
                SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper,
            },
            general_messages::{DroneSend, ServerType, ServerTypeRequest, ServerTypeResponse},
        },
    };
    use wg_2024::{
        network::SourceRoutingHeader,
        packet::{Packet, PacketType},
    };

    use crate::tests::utils::build_server;

    #[test]
    fn process_file_request() {
        let (mut server, neighbor, _, sim_controller_response) = build_server();

        let type_request = BrowserRequestWrapper::ServerType(ServerTypeRequest::ServerType);
        let type_request_json = type_request.stringify();

        let disassembled =
            Disassembler::new().disassemble_message(type_request_json.as_bytes().to_vec(), 0);

        let packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![21, 2, 1], 1),
            session_id: 0,
            pack_type: PacketType::MsgFragment(disassembled.get(0).unwrap().clone()),
        };

        server.handle_drone_packets(Ok(packet));

        let ack_packet = neighbor.1.recv().unwrap();
        println!("Received ACK packet: {:?}", ack_packet);

        match ack_packet.pack_type {
            PacketType::Ack(ack) => {
                assert_eq!(ack.fragment_index, 0, "Incorrect ACK fragment index");
            }
            _ => panic!("The first packet received is not an ACK"),
        }

        let received_packet = neighbor.1.recv().unwrap();

        let expected_response =
            BrowserResponseWrapper::ServerType(ServerTypeResponse::ServerType(ServerType::Text));

        let expected_response_json = expected_response.stringify();

        let disassembled_response =
            Disassembler::new().disassemble_message(expected_response_json.as_bytes().to_vec(), 0);

        let expected_packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![1, 2, 21], 1),
            session_id: received_packet.session_id,
            pack_type: PacketType::MsgFragment(disassembled_response.get(0).unwrap().clone()),
        };

        assert_eq!(expected_packet, received_packet, "Do not correspond");

        assert!(server
            .sent_packets
            .contains_key(&expected_packet.session_id));

        let sim_controller_message = sim_controller_response.1.recv().unwrap();
        match sim_controller_message {
            SimControllerResponseWrapper::Event(event) => match event {
                SimControllerEvent::MessageSent { session_id } => {
                    assert_eq!(session_id, expected_packet.session_id);
                }
                _ => panic!("Print 1"),
            },
            _ => panic!("Print 2"),
        }
    }
}
