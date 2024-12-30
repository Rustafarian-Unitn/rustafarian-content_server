use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::{env, fs, thread};
use image::{DynamicImage, GenericImageView, ImageFormat};
use rustafarian_shared::assembler::{assembler::Assembler, disassembler::Disassembler};
use rustafarian_shared::messages::browser_messages::{BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper};
use rustafarian_shared::messages::commander_messages::{
    SimControllerCommand, SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper
};
use rustafarian_shared::messages::general_messages::{DroneSend, Message, Request, Response, ServerType, ServerTypeResponse};
use rustafarian_shared::topology::{compute_route, Topology};
use rustafarian_shared::TIMEOUT_TIMER_MS;
use wg_2024::controller::DroneEvent;
use wg_2024::packet::{self, Ack, Fragment, Nack, NackType, NodeType};
use wg_2024::{
    network::*,
    packet::{FloodRequest, FloodResponse, Packet, PacketType},
};

use crossbeam_channel::{select_biased, Receiver, Sender};


pub struct ContentServer{
    server_id: u8,
    senders: HashMap<u8, Sender<Packet>>,
    receiver: Receiver<Packet>,
    pub topology: Topology,
    sim_controller_receiver: Receiver<SimControllerCommand>,
    sim_controller_sender: Sender<SimControllerResponseWrapper>,
    sent_packets: HashMap<u64, Packet>,
    assembler: Assembler,
    deassembler: Disassembler,
    files:HashMap<u8, String>,
    media:HashMap<u8, String>,
    server_type: ServerType,
    nack_queue: Vec<Nack>,
}


impl ContentServer {


    /*
        Create a new server by checking the type and initializing the corresponding hashmap to manage the files
    */
    pub fn new(
        server_id: u8,
        senders: HashMap<u8, Sender<Packet>>,
        receiver: Receiver<Packet>,
        sim_controller_receiver: Receiver<SimControllerCommand>,
        sim_controller_sender: Sender<SimControllerResponseWrapper>,
        file_directory: &str, 
        media_directory: &str,
        server_type: ServerType
    )->Self {
        let current_dir = env::current_dir().expect("Failed to get current directory");

        let mut files = HashMap::new();
        let mut media = HashMap::new();

        match server_type{
            ServerType::Text=>{
                let file_path = current_dir.join(file_directory);
                if !file_path.exists() {
                    eprintln!("Error: File directory '{}' does not exist!", file_path.display());
                    std::process::exit(1); 
                }
                if let Ok(entries) = fs::read_dir(file_path) {
                    for (id, entry) in entries.filter_map(Result::ok).enumerate() {
                        if let Some(path) = entry.path().to_str() {
                            files.insert(id as u8, path.to_string());
                        }
                    }
                }
        
            }
            ServerType::Media=>{
                let media_path = current_dir.join(media_directory);
                if !media_path.exists() {
                    eprintln!("Error: Media directory '{}' does not exist!", media_path.display());
                    std::process::exit(1); 
                }
        
                if let Ok(entries) = fs::read_dir(media_path) {
                    for (id, entry) in entries.filter_map(Result::ok).enumerate() {
                        if let Some(path) = entry.path().to_str() {
                            media.insert(id as u8, path.to_string());
                        }
                    }
                }
            }
            ServerType::Chat=>{
                eprintln!("Error: ServerType::Chat is not supported!");
                std::process::exit(1);  
            }
        }
       
        ContentServer{
            server_id,
            senders,
            receiver,
            topology: Topology::new(),
            sim_controller_receiver,
            sim_controller_sender,
            sent_packets: HashMap::new(),
            assembler: Assembler::new(),
            deassembler: Disassembler::new(),
            files,
            media,
            server_type,
            nack_queue:Vec::new()
        }
    }


    /*
        Returns the list of files
    */
    pub fn handle_files_list(&mut self, source_id: NodeId, session_id: u64, route:Vec<u8>){
        let file_ids: Vec<u8> = self.files.keys().cloned().collect();

        let request=BrowserResponseWrapper::Chat(BrowserResponse::FileList(file_ids));
        let request_json=request.stringify();
        println!("Request Json -> {:?}",request_json);
        self.send_message(source_id, request_json, session_id, route );
    }

    /*
        Returns a text 
    */
    pub fn handle_file_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        if let Some(file_path)=self.files.get(&id){

            match fs::read(file_path) {
                Ok(file_data)=>{
                    let file_string = match String::from_utf8(file_data) {
                        Ok(string) => string,
                        Err(err) => {
                            eprintln!("Error converting file data to String: {}", err);
                            return; 
                        }
                    };
                    let request=BrowserResponseWrapper::Chat(BrowserResponse::TextFile(id, file_string));
                    let request_json=request.stringify();
                    self.send_message(source_id, request_json, session_id, route);
                }
                Err(e)=>{
                    eprintln!("Error reading file '{}': {}", file_path, e);
                }
            }
        } else {
            eprintln!("File with ID '{}' not found", id);
        }
    }

    /*
        Returns an media 
    */
    pub fn handle_media_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        if let Some(media_path)=self.media.get(&id){

            match image::open(media_path) {
                Ok(image)=>{
                    
                    let gray_image = image.to_luma8();

                    let mut buffer = Vec::new();
                    match gray_image.write_to(&mut Cursor::new(&mut buffer), ImageFormat::Jpeg) {
                        Ok(_) => {
                            
                            let request = BrowserResponseWrapper::Chat(BrowserResponse::MediaFile(id, buffer));
                            let request_json = request.stringify();
                            self.send_message(source_id, request_json, session_id, route);
                        }
                        Err(e) => {
                            eprintln!("Error in image: {}", e);
                        }
                    }
                }
                Err(e)=>{
                    eprintln!("Error reading media '{}': {}", media_path, e);
                }
            }
        } else {
            eprintln!("Media with ID '{}' not found", id);
        }
    }

    
    
    /*
        Returns the type of the server
    */
   pub fn handle_type_request(&mut self, source_id:NodeId, session_id:u64, route:Vec<u8>) {
    let request=BrowserResponseWrapper::ServerType(ServerTypeResponse::ServerType(self.server_type.clone()));
    let request_json=request.stringify();
    self.send_message(source_id, request_json, session_id, route);
   }


    /*
        Send a message by dividing it into packets using desassembler
    */
    fn send_message(&mut self, destination_id: u8, message: String, session_id: u64, route: Vec<u8>) {
        let fragments = self
            .deassembler
            .disassemble_message(message.as_bytes().to_vec(), session_id);
        let server_id = self.server_id;
        
        for fragment in fragments {

            let packet = Packet {
                pack_type: PacketType::MsgFragment(fragment),
                session_id,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: route.iter().rev().cloned().collect(),
                },
            };

            self.sent_packets
            .insert(packet.session_id, packet.clone());
            let drone_id = packet.routing_header.hops[1];
            match self.senders.get(&drone_id) {
                Some(sender) => {
                    sender.send(packet.clone()).unwrap();
                    let _res=self.sim_controller_sender.send(SimControllerResponseWrapper::Event(
                        SimControllerEvent::PacketSent { session_id: packet.session_id.clone(), packet_type: packet.pack_type.to_string() }
                    ));
                }
                None => {
                    eprintln!(
                        "Server {}: No sender found for client {}",
                        self.server_id,
                        drone_id
                    );
                }
            }
        }
    }

    
    fn handle_controller_commands(&mut self, command: SimControllerCommand) {
       match command {
           SimControllerCommand::AddSender(id, channel )=>{
                self.senders.insert(id, channel);
           }
           SimControllerCommand::RemoveSender(id)=>{
                self.senders.remove(&id);
           }    
           _=>{
            println!("Client commands")
           }
       }
    }
    

    /*
       Handles requests based on server type and command
    */
    fn process_request(
        &mut self,
        source_id: NodeId,
        session_id: u64,
        raw_content: String,
        route:Vec<u8>
    ) {
        
        match BrowserRequestWrapper::from_string(raw_content) {
            Ok(request_wrapper) => {
                
                match request_wrapper {
                    BrowserRequestWrapper::Chat(request)=>{
                        match request {
                            BrowserRequest::FileList => self.handle_files_list(source_id, session_id, route),
                            BrowserRequest::TextFileRequest(id) => {
                                match self.server_type {
                                    ServerType::Text=>{
                                        self.handle_file_request(id, source_id, session_id, route)
                                    }
                                    _=>{
                                        eprintln!("This server cannot handle text file requests");
                                    }
                                }
                            }
                            BrowserRequest::MediaFileRequest(id) => {
                                match self.server_type {
                                    ServerType::Media=>{
                                        self.handle_media_request(id, source_id, session_id, route)
                                    }
                                    _=>{
                                        eprintln!("This server cannot handle media file requests");
                                    }
                                }
                            }
                        } 
                    }
                    BrowserRequestWrapper::ServerType(_request)=>{
                        self.handle_type_request(source_id, session_id, route)
                    }
                }

                
            }
            Err(err) => {
                eprintln!("Error deserializing request: {}", err);
            }
        }
    }
    
    
    /*
        Function to manage various types of packets
     */
    pub fn handle_drone_packets(&mut self, packet: Result<Packet, crossbeam_channel::RecvError>) {
        match packet {
            Ok(packet) => {
                match &packet.pack_type {
                    
                    // arrival of a fragment
                    PacketType::MsgFragment(fragment) => {
                        // sends acknowledgment that packet has been received
                        self.send_ack(fragment.fragment_index, packet.session_id, packet.routing_header.source().expect("Missing source ID in routing header"));
                        let _res = self
                        .sim_controller_sender
                        .send(SimControllerResponseWrapper::Event(
                            SimControllerEvent::PacketReceived(packet.session_id),
                        ));
                        // if the message is complete, proceed to handle the request
                        if let Some(message) =
                            self.assembler.add_fragment(fragment.clone(), packet.session_id)
                        {
                            let message_str = String::from_utf8_lossy(&message);
                            self.process_request(
                                packet.routing_header.source().expect("Missing source ID in routing header"),
                                packet.session_id,
                                message_str.to_string(),
                                packet.routing_header.hops
                            );
                        }
                    }
                    // arrival of a flood response
                    PacketType::FloodResponse(flood_response) => {
                        self.on_flood_response(flood_response.clone());
                    }
                    // arrival of an ack
                    PacketType::Ack(ack)=>{
                        self.on_ack_arrived(ack.clone());
                    }
                    // arrival of a nack
                    PacketType::Nack(nack)=>{
                        self.on_nack_arrived(nack.clone());
                    }
                    // arrival of a flood request
                    PacketType::FloodRequest(flood_request)=>{
                        self.on_flood_request(packet.clone(), flood_request.clone());
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "Server {}: Error receiving packet: {:?}",
                    self.server_id,
                    err
                );
            }
        }
    }

    /*
       When an ack arrives for a sent packet, the corresponding packet is removed from sent_packets
     */
    fn on_ack_arrived(&mut self, ack: Ack) {
        if let Some(session_id) = self.sent_packets
            .iter()
            .find_map(|(session_id, packet)| {
                match &packet.pack_type {
                    PacketType::MsgFragment(fragment) if fragment.fragment_index == ack.fragment_index => {
                        Some(*session_id)
                    }
                    _ => None,
                }
            })
        {
            self.sent_packets.remove(&session_id);
            println!("Removed packet with session ID: {}", session_id);
        } else {
            eprintln!("No matching packet found for Ack with fragment index: {}", ack.fragment_index);
        }
    }


    /*
       When a nack arrives the type is checked and the corresponding actions are performed and the packet is resent
     */
    fn on_nack_arrived(&mut self, nack: Nack) {

        
        match nack.nack_type {
            // Resend packet
            NackType::Dropped=>{
                self.resend_packet(nack.fragment_index);
            }
            // Remove node, execute flood request and resend packet
            NackType::ErrorInRouting(node_id)=>{
                self.nack_queue.push(nack);
                self.topology.remove_node(node_id);
                self.send_flood_request();
                self.resend_nacks_in_queue();
            }
            // Remove node, execute flood request and resend packet
            _=>{
                self.nack_queue.push(nack);
                self.send_flood_request();
                self.resend_nacks_in_queue();
            }
        }
        
    }



    fn resend_nacks_in_queue(&mut self) {
        while let Some(nack)=self.nack_queue.pop() {
            self.resend_packet(nack.fragment_index);
        }
    }

    /*
        Searches for a packet corresponding to a fragment index, resends it by computing the new topology
    */
    fn resend_packet(&mut self, fragment_index: u64) {
        if let Some(session_id) = self.sent_packets
            .iter()
            .find_map(|(session_id, packet)| {
                match &packet.pack_type {
                    PacketType::MsgFragment(fragment) if fragment.fragment_index == fragment_index => {
                        Some(*session_id)
                    }
                    _ => None,
                }
            })
        {
            if let Some(packet) = self.sent_packets.get_mut(&session_id) {
                println!("Found packet with session ID: {}", session_id);   

                let destination_id=packet.routing_header.hops[packet.routing_header.hops.len() - 1];

                let new_routing=compute_route(&mut self.topology, self.server_id, destination_id);
                packet.routing_header.hops=new_routing;
                let drone_id = packet.routing_header.hops[1];
                match self.senders.get(&drone_id) {
                    Some(sender) => {
                        sender.send(packet.clone()).unwrap_or_else(|err| {
                            eprintln!("Failed to resend packet: {}", err);
                        });

                        let _res = self
                        .sim_controller_sender
                        .send(SimControllerResponseWrapper::Event(
                            SimControllerEvent::PacketSent {
                                session_id,
                                packet_type: packet.pack_type.to_string(),
                            },
                        ));
                    }
                    None => {
                        eprintln!(
                            "Server {}: No sender found for client {}",
                            self.server_id,
                            drone_id
                        );
                    }
                }
            }
        } else {
            eprintln!(
                "No matching packet found for Nack with fragment index: {}",
                fragment_index
            );
        }
    }


    /*
        If a flood request arrives it adds itself and sends it to the neighbors from which it did not arrive
    */
    fn on_flood_request(&mut self, packet: Packet, mut request: FloodRequest) {
        let sender_id = request.path_trace.last().unwrap().0;
        request.increment(self.server_id, NodeType::Server);
        let response = Packet::new_flood_request(
            SourceRoutingHeader::empty_route(),
            packet.session_id,
            request,
        );
        for (neighbor_id, sender) in self.senders.clone() {
            if neighbor_id != sender_id {
                sender.send(response.clone()).unwrap();
            }
        }
    }

    /*
        When a flood response arrives, it checks the route and adds the corresponding nodes to the topology
    */
    fn on_flood_response(&mut self, flood_response: FloodResponse) {
        println!(
            "Server {} received FloodResponse: {:?}",
            self.server_id,
            flood_response
        );
        for (i, node) in flood_response.path_trace.iter().enumerate() {
            if !self.topology.nodes().contains(&node.0) {
                self.topology.add_node(node.0);
            }
            if i > 0 {
                if self
                    .topology
                    .edges()
                    .get(&node.0)
                    .unwrap()
                    .contains(&flood_response.path_trace[i - 1].0)
                {
                    continue;
                }
                self.topology
                    .add_edge(flood_response.path_trace[i - 1].0, node.0);
                self.topology
                    .add_edge(node.0, flood_response.path_trace[i - 1].0);
            }
        }

        let _res = self
            .sim_controller_sender
            .send(SimControllerResponseWrapper::Message(
                SimControllerMessage::FloodResponse(flood_response.flood_id),
            ));
    }


    /* 
        Manages packets sent by the simulation controller
    */
    fn handle_sim_controller_packets(
        &mut self,
        packet: Result<SimControllerCommand, crossbeam_channel::RecvError>,
    ) {
        match packet {
            Ok(packet) => self.handle_controller_commands(packet),
            Err(err) => {
                eprintln!(
                    "Server {}: Error receiving packet from the simulation controller: {:?}",
                    self.server_id,
                    err
                );
            }
        };
    }

    /*
        Keeps the server active
    */
    pub fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.sim_controller_receiver) -> packet => {
                    self.handle_sim_controller_packets(packet);
                }
                recv(self.receiver) -> packet => {
                    self.handle_drone_packets(packet);
                }
            }
        }
    }

    /*
        Send a flood request to neighbors
    */
    fn send_flood_request(&mut self) {
        for sender in &self.senders {
            let packet = Packet {
                pack_type: PacketType::FloodRequest(FloodRequest {
                    initiator_id: self.server_id,
                    flood_id: rand::random(),
                    path_trace: Vec::new(),
                }),
                session_id: rand::random(),
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: Vec::new(),
                },
            };
            sender.1.send(packet).unwrap();
        }
        self.sim_controller_sender
            .send(SimControllerResponseWrapper::Event(
                SimControllerEvent::FloodRequestSent,
            ))
            .unwrap();
    }



    /*
        Sends an ack with confirmations to a received packet
    */
    fn send_ack(&mut self, fragment_index:u64, session_id: u64, destination_id: u8) {
        let packet=Packet{
            pack_type:PacketType::Ack(Ack { fragment_index }),
            session_id:session_id,
            routing_header:SourceRoutingHeader {
                hop_index: 1,
                hops: compute_route(&mut self.topology, self.server_id, destination_id),
            },
        };

        let drone_id = packet.routing_header.hops[1];
            match self.senders.get(&drone_id) {
                Some(sender) => {
                    sender.send(packet).unwrap();
                }
                None => {
                    eprintln!(
                        "Server {}: No sender found for client {}",
                        self.server_id,
                        drone_id
                    );
                }
            }



    }
}



