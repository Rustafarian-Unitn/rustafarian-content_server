
#[cfg(test)]
#[allow(unused)]
pub mod file_media_request_test {
    use std::{fs, io::{Cursor, Read}};

    use image::GenericImageView;
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

        println!("File ID selezionato: {}", 2);
        let file_request = BrowserRequestWrapper::Chat(BrowserRequest::MediaFileRequest(3));
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
                            
                            
                            if let Some(reassembled_data) = assembler.add_fragment(fragment.clone(), received_packet.session_id) {
                                println!("Lenght {}",reassembled_data.len());
                                
                                let response_json = String::from_utf8(reassembled_data)
                                    .expect("Errore nella decodifica del messaggio JSON");
                                //println!("Messaggio ricevuto: {}", response_json);
                    
                                let response: BrowserResponseWrapper =
                                    serde_json::from_str(&response_json)
                                        .expect("Errore nella deserializzazione del JSON");
                    
                                match response {
                                    BrowserResponseWrapper::Chat(BrowserResponse::MediaFile(id, content)) => {
                                        let cursor = Cursor::new(content);

                                        // Decodes the image bytes
                                        match image::load(cursor, image::ImageFormat::Jpeg) {
                                            Ok(image) => {
            
                                                // Save the image or do what you want
                                                image.save("received_image.jpg").unwrap();
                                            }
                                            Err(e) => {
                                                eprintln!("Error decoding the image: {}", e);
                                            }
                                        }
                    
                                        
                                        break;
                                    }
                                    _ => println!("Risposta del server non del tipo previsto"),
                                }
                            } else {
                                
                                //println!("Il frammento è stato aggiunto, ma l'assemblaggio non è ancora completo.");
                            }
                        }
                        _ => println!("Tipo di pacchetto sconosciuto ricevuto"),
                    }
                }
                Err(_) => {
                    println!("Nessun altro pacchetto da ricevere, terminazione ciclo.");
                    break;
                }
            }
        }
}
}