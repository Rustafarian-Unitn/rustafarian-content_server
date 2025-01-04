
#[cfg(test)]
#[allow(unused)]
pub mod fragment_dropped_test {
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
        packet::{Nack, NackType, Packet, PacketType},
    };

    use crate::tests::utils::build_server;

    


#[test]
    fn fragment_dropped_test() {
        let (mut server, neighbor, _, _) = build_server();

        
        let (&file_id, file_content) = server.files.iter().next().expect("Nessun file disponibile nel server.");
        println!("File ID selezionato: {}", 2);
        let file_request = BrowserRequestWrapper::Chat(BrowserRequest::TextFileRequest(2));
        let file_request_json = file_request.stringify();

        let disassembled =
            Disassembler::new().disassemble_message(file_request_json.as_bytes().to_vec(), 0);

        let packet = Packet {
            routing_header: SourceRoutingHeader::new(vec![21, 2, 1], 1),
            session_id: 12345, 
            pack_type: PacketType::MsgFragment(disassembled.get(0).unwrap().clone()),
        };

        
        server.handle_drone_packets(Ok(packet));

        let mut assembler = Assembler::new();
    loop {
        match neighbor.1.recv() {
            Ok(received_packet) => {
                //println!("Ricevuto pacchetto: {:?}", received_packet);

                match received_packet.pack_type {
                    PacketType::MsgFragment(fragment) => {
                        //println!("Frammento n {} di {} con session_id {}",fragment.fragment_index,fragment.total_n_fragments, received_packet.session_id );
                        
                        println!("total_frag={}",fragment.total_n_fragments);
                        if let Some(reassembled_data) = assembler.add_fragment(fragment.clone(), received_packet.session_id) {
                            println!("Lenght {}",reassembled_data.len());
                            
                            let response_json = String::from_utf8(reassembled_data)
                                .expect("Errore nella decodifica del messaggio JSON");
                            //println!("Messaggio ricevuto: {}", response_json);
                
                            let response: BrowserResponseWrapper =
                                serde_json::from_str(&response_json)
                                    .expect("Errore nella deserializzazione del JSON");
                
                            match response {
                                BrowserResponseWrapper::Chat(BrowserResponse::TextFile(id, content)) => {
                                    println!("TextFile id {} con contenuto {}", id, content);
                
                                    let expected_file_content = "This is the text number 2 Lorem ipsum dolor sit amet, consectetur adipiscing elit. Vestibulum ultrices faucibus tincidunt. Donec volutpat euismod fermentum.\r\n";
                                    assert_eq!(
                                        content, expected_file_content,
                                        "Il contenuto del file non corrisponde a quanto previsto"
                                    );
                                    break;
                                    
                                }
                                _ => println!("Risposta del server non del tipo previsto"),
                            }
                        } else {
                            
                            println!("Il frammento è stato aggiunto, ma l'assemblaggio non è ancora completo.");
                        }
                    }
                    _ => println!("Tipo di pacchetto sconosciuto ricevuto {}", received_packet),
                }
            }
            Err(_) => {
                println!("Nessun altro pacchetto da ricevere, terminazione ciclo.");
                break;
            }
        }
    }

    // Check list of packet
    if let Some(packets_stored) =server.sent_packets.get(&12345){
        for pack in packets_stored{
            println!("server.sent_packet id={:?},index={:?}",pack.session_id,pack.get_fragment_index());
        }
    }   
    

    let nack1=Nack{
        fragment_index:1,
        nack_type:NackType::Dropped,
    };
    let nack_packet1=Packet{
        routing_header:SourceRoutingHeader{
            hop_index:1,
            hops:vec![2,1],
        },
        session_id:12345,
        pack_type:PacketType::Nack(nack1),
    };

    let nack2=Nack{
        fragment_index:0,
        nack_type:NackType::Dropped,
    };
    let nack_packet2=Packet{
        routing_header:SourceRoutingHeader{
            hop_index:1,
            hops:vec![2,1],
        },
        session_id:12345,
        pack_type:PacketType::Nack(nack2),
    };

    server.handle_drone_packets(Ok(nack_packet1));
    server.handle_drone_packets(Ok(nack_packet2));


    loop {
        match neighbor.1.recv() {
            Ok(received_packet) => {

                match received_packet.pack_type {
                    PacketType::MsgFragment(fragment) => {
                        println!("Frammento n {} di {} con session_id {}",fragment.fragment_index,fragment.total_n_fragments, received_packet.session_id ); 
                             
                    }
                    _=>println!("Non riconosciuto")
            }
            
        }
            Err(_) => todo!(),
    }
}
}
}