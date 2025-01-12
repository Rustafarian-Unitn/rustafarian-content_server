#[cfg(test)]
#[allow(unused)]
pub mod file_list_request_test {
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
        network::SourceRoutingHeader,
        packet::{Fragment, Packet, PacketType},
    };

    use crate::tests::utils::build_server;

    #[test]
    fn file_list_request_test() {
        let (mut server, neighbor, _, _) = build_server();

        let file_request = BrowserRequestWrapper::Chat(BrowserRequest::FileList);
        let file_request_json = file_request.stringify();

        let disassembled =
            Disassembler::new().disassemble_message(file_request_json.as_bytes().to_vec(), 0);

        let packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![21, 2, 1], 1),
            session_id: 2,
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
        let expected_ids: Vec<u8> = server.files.keys().cloned().collect();
        let expected_response =
            BrowserResponseWrapper::Chat(BrowserResponse::FileList(expected_ids.clone()));

        let expected_response_json = expected_response.stringify();

        let disassembled_response =
            Disassembler::new().disassemble_message(expected_response_json.as_bytes().to_vec(), 0);

        let expected_packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![1, 2, 21], 1),
            session_id: received_packet.session_id,
            pack_type: PacketType::MsgFragment(disassembled_response.get(0).unwrap().clone()),
        };

        println!("Expected ids {:?}", expected_ids);
        assert_eq!(expected_packet, received_packet, "Do not correspond");
    }
}
