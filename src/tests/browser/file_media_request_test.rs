
#[cfg(test)]
#[allow(unused)]
pub mod file_media_request_test {
    use rustafarian_shared::{
        assembler::{assembler::Assembler, disassembler::Disassembler},
        messages::{
            browser_messages::{BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper},
            commander_messages::{SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper},
            general_messages::{DroneSend, ServerType, ServerTypeResponse},
        },
    };
    use wg_2024::{
        network::SourceRoutingHeader,
        packet::{Packet, PacketType},
    };

    use crate::tests::utils::build_server;


#[test]
    fn file_media_request_test() {
        let (mut server, neighbor, _, _) = build_server();

        
        let file_id = 1; 
        let file_request = BrowserRequestWrapper::Chat(BrowserRequest::MediaFileRequest(file_id));
        let file_request_json = file_request.stringify();

        let disassembled =
            Disassembler::new().disassemble_message(file_request_json.as_bytes().to_vec(), 0);

        let packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![21, 2, 1], 1),
            session_id: 12345, 
            pack_type: PacketType::MsgFragment(disassembled.get(0).unwrap().clone()),
        };

        
        server.handle_drone_packets(Ok(packet));

        
        let ack_packet = neighbor.1.recv().unwrap();
        match ack_packet.pack_type {
            PacketType::Ack(ack) => {
                assert_eq!(ack.fragment_index, 0, "Fragment index ACK not right");
            }
            _ => panic!("First packet not ack"),
        }

        
        let expected_file_content = "File content requested.";
        let received_packet = neighbor.1.recv().unwrap();

        match received_packet.pack_type {
            PacketType::MsgFragment(fragment) => {
               
                let reassembled = Assembler::new().add_fragment(fragment.clone(), received_packet.session_id);
                let response_json = String::from_utf8(reassembled.unwrap()).expect("Error decoding JSON message");

                
                let response: BrowserResponseWrapper =
                    serde_json::from_str(&response_json).expect("Error deserializing JSON");

                match response {
                    BrowserResponseWrapper::Chat(BrowserResponse::TextFile(id, content)) => {

                        println!("Text with id {} with text {}", id, content);
                        assert_eq!(
                            content, expected_file_content,
                            "The contents of the file do not match what was expected"
                        );
                    }
                    _ => panic!("Server response is not of the expected type"),
                }
            }
            _ => panic!("The second packet received is not a message fragment"),
        }
    }

}