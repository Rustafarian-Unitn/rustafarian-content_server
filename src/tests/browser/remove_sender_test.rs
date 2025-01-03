#[cfg(test)]
#[allow(unused)]
pub mod remove_sender_test {
    use crossbeam::utils;
    use crossbeam_channel::{unbounded, Sender};
    use rustafarian_shared::{
        assembler::{assembler::Assembler, disassembler::Disassembler},
        messages::{
            browser_messages::{BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper},
            commander_messages::{SimControllerCommand, SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper},
            general_messages::{DroneSend, ServerType, ServerTypeResponse},
        },
    };
    use wg_2024::{
        network::SourceRoutingHeader,
        packet::{Packet, PacketType}, tests,
    };

    use crate::tests::utils::build_server;


    #[test]
    fn remove_sender_test() {

        let (
            mut server,
            _neighbor,
            _controller_channel_commands,
            _controller_channel_messages,
        ) = build_server();
        
        let as_request = SimControllerCommand::RemoveSender(2);

        server.handle_sim_controller_packets(Ok(as_request));

        assert!(!server.senders.contains_key(&2));

        assert!(!server.topology.nodes().contains(&2));
        assert!(server.topology.edges().contains_key(&1));
        assert!(!server.topology.edges().contains_key(&2));
        assert!(!server.topology.edges().get(&1).unwrap().contains(&2));
    }



}