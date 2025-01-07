use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::str::FromStr;
use std::{env, fs};
use chrono::Utc;
use log::{debug, error, info, LevelFilter, Record};
use env_logger::Builder;
use image::ImageFormat;
use rand::seq::SliceRandom;
use rustafarian_shared::assembler::{assembler::Assembler, disassembler::Disassembler};
use rustafarian_shared::TIMEOUT_BETWEEN_FLOODS_MS;
use rustafarian_shared::messages::browser_messages::{BrowserRequest, BrowserRequestWrapper, BrowserResponse, BrowserResponseWrapper};
use rustafarian_shared::messages::commander_messages::{
    SimControllerCommand, SimControllerEvent, SimControllerMessage, SimControllerResponseWrapper
};
use rustafarian_shared::messages::general_messages::{DroneSend, ServerType, ServerTypeResponse};
use rustafarian_shared::topology::{compute_route, Topology};
use wg_2024::packet::{Ack, Nack, NackType, NodeType};
use wg_2024::{
    network::*,
    packet::{FloodRequest, FloodResponse, Packet, PacketType},
};

use crossbeam_channel::{select_biased, Receiver, Sender};


pub enum LogLevel {
    INFO,
    DEBUG,
    ERROR,
}

pub struct ContentServer{
    server_id: u8,
    pub senders: HashMap<u8, Sender<Packet>>,
    receiver: Receiver<Packet>,
    pub topology: Topology,
    sim_controller_receiver: Receiver<SimControllerCommand>,
    sim_controller_sender: Sender<SimControllerResponseWrapper>,
    pub sent_packets: HashMap<u64, Vec<Packet>>,
    assembler: Assembler,
    deassembler: Disassembler,
    pub files:HashMap<u8, String>,
    media:HashMap<u8, String>,
    server_type: ServerType,
    pub nack_queue: HashMap<u64, Vec<Nack>>,
    flood_time: u128,
    is_debug: bool
}



impl ContentServer {

    /// Returns a instance of ContentServer
    pub fn new(
        server_id: u8,
        senders: HashMap<u8, Sender<Packet>>,
        receiver: Receiver<Packet>,
        sim_controller_receiver: Receiver<SimControllerCommand>,
        sim_controller_sender: Sender<SimControllerResponseWrapper>,
        file_directory: &str, 
        media_directory: &str,
        server_type: ServerType,
        is_debug: bool
    )->Self {

        
        // Retrieves current directory
        let current_dir = env::current_dir().expect("Failed to get current directory");

        // Initialize hashmaps for files and media
        let mut files = HashMap::new();
        let mut media = HashMap::new();

        // Configure based on the server type 
        match server_type{
            // If it's a text server upload text files
            ServerType::Text=>{
                let file_path = current_dir.join(file_directory);
                // Check if the directory exist
                if !file_path.exists() {
                    
                    error!("Error: File directory '{}' does not exist!\n", file_path.display());
                    std::process::exit(1); 
                }
                let mut file_list = Vec::new();
                // Reads files from directory and places them in files hashmap
                if let Ok(entries) = fs::read_dir(file_path) {
                    for entry in entries.filter_map(Result::ok) {
                        if let Some(path) = entry.path().to_str() {
                            if let Some(filename) = entry.file_name().to_str() {
                                // Check extension
                                if filename.ends_with(".txt") {
                                    // Parse the numeric character
                                    if let Ok(id) = filename.trim_end_matches(".txt").parse::<u8>() {
                                        file_list.push((id, path.to_string()));
                                    } else {
                                        error!("Warning: Failed to parse ID from filename '{}'\n", filename);
                                    }
                                }
                            }
                        }
                    }
                }
                // Select only 10 random
                let mut rng = rand::thread_rng();
                //file_list.shuffle(&mut rng); 
                let selected_files = file_list.into_iter().take(10);
                for (id, path) in selected_files {
                    files.insert(id, path);
                }
        
            }
            // If it's a media server upload media files
            ServerType::Media=>{
                let media_path = current_dir.join(media_directory);
                // Check if the directory exist
                if !media_path.exists() {
                    error!("Error: Media directory '{}' does not exist!\n", media_path.display());
                    std::process::exit(1); 
                }
                // Reads files from directory and places them in media hashmap
                let mut media_list = Vec::new();
                if let Ok(entries) = fs::read_dir(media_path) {
                    for entry in entries.filter_map(Result::ok) {
                        if let Some(path) = entry.path().to_str() {
                            if let Some(file_name) = entry.file_name().to_str() {
                                if file_name.ends_with(".jpg") {
                                    // Parse name
                                    if let Ok(id) = file_name.trim_end_matches(".jpg").parse::<u8>() {
                                        media_list.push((id, path.to_string()));
                                    } else {
                                        error!("Unable to parse ID from file name '{}'\n", file_name);
                                    }
                                }
                            }
                        }
                    }
                }
                // Select only 10
                let mut rng = rand::thread_rng();
                //media_list.shuffle(&mut rng);
                let selected_media = media_list.into_iter().take(10);
                for (id, path) in selected_media {
                    media.insert(id, path);
                }
            }
            // If it's a chat server gives error
            ServerType::Chat=>{
                error!("Error: ServerType::Chat is not supported!\n");
                std::process::exit(1);  
            }
        }
        
        // Create and return a new instance of ContentServer
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
            nack_queue:HashMap::new(),
            flood_time:0,
            is_debug,
        }
    }

    /// Keeps the server active and continuously listens to two main channels
    pub fn run(&mut self) {

        self.log(format!("Server {} is running\n", self.server_id).as_str(),LogLevel::INFO);
        // Send a flood request to obtain the initial topology
        self.send_flood_request();
        loop {
            select_biased! {
                // Receives a command from the simulator
                recv(self.sim_controller_receiver) -> packet => {
                    self.handle_sim_controller_packets(packet);
                }
                // Receives a command from the drones
                recv(self.receiver) -> packet => {
                    self.handle_drone_packets(packet);
                }
            }

            // Resend packet that are waiting for a new path
            if !self.nack_queue.is_empty(){
                self.resend_nacks_in_queue();
            }

        }
    }
    
    /// Receive packets from the controller channel and handle them
    pub fn handle_sim_controller_packets(&mut self, packet: Result<SimControllerCommand, crossbeam_channel::RecvError>,) {
        match packet {
            // If packet is valid match its type
            Ok(command) => {
                match command {
                    // Add a drone as sender
                    SimControllerCommand::AddSender(id, channel )=>{
                        self.log(format!("Server {} received addsender\n", self.server_id).as_str(), LogLevel::INFO);
                        self.handle_add_sender(id, channel);
                    }
                    // Remove a drone from senders
                    SimControllerCommand::RemoveSender(id)=>{
                        self.log(format!("Server {} received removesender\n", self.server_id).as_str(),LogLevel::INFO);
                        self.handle_remove_sender(id);
                    }   
                    // Send server topology
                    SimControllerCommand::Topology=>{
                        self.handle_topology_request();
                    } 
                    // Other commands are not for this type of server
                    _=>{
                        self.log(format!("Client commands\n").as_str(),LogLevel::ERROR);
                    }
                }
            },
            // If there is an error print error
            Err(err) => {
                self.log(format!(
                    "Server {}: Error receiving packet from the simulation controller: {:?}\n",
                    self.server_id,
                    err).as_str(),LogLevel::ERROR
                );
            }
        };
    }


    fn handle_add_sender(&mut self, id: NodeId, channel: Sender<Packet>) {
        self.senders.insert(id, channel);
        self.topology.add_edge(self.server_id, id);
    }

    fn handle_remove_sender(&mut self, id: NodeId) {
        self.senders.remove(&id);
        self.topology.remove_edges(self.server_id, id);
    }

    fn handle_topology_request(&mut self) {
        self.log(format!("Server {} received topology request\n", self.server_id).as_str(),LogLevel::INFO);

        let topology_response=SimControllerResponseWrapper::Message(
            SimControllerMessage::TopologyResponse(self.topology.clone()));
        self.sim_controller_sender.send(topology_response).unwrap();
    }

    /// Receive packets from the drones channels and handle them
    pub fn handle_drone_packets(&mut self, packet: Result<Packet, crossbeam_channel::RecvError>) {
        match packet {
            // If packet is valid match its type
            Ok(packet) => {
                match &packet.pack_type {
                    // Packet is a message fragment
                    PacketType::MsgFragment(fragment) => {
                        self.log(format!("Server {} received fragment {}\n", self.server_id, packet.get_fragment_index()).as_str(),LogLevel::DEBUG);
                        // Sends ACK that packet has been received
                        self.send_ack(fragment.fragment_index, packet.session_id, packet.routing_header.hops.clone());
                        // Notify controller that the packet has been received
                        let _res = self
                        .sim_controller_sender
                        .send(SimControllerResponseWrapper::Event(
                            SimControllerEvent::PacketReceived(packet.session_id),
                        ));
                        // If message is complete pass it to 'process_request'
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
                    // Packet is a flood response
                    PacketType::FloodResponse(flood_response) => {
                        self.on_flood_response(flood_response.clone(), packet.clone());
                    }
                    // Packet is a flood request
                    PacketType::FloodRequest(flood_request)=>{
                        self.on_flood_request(packet.clone(), flood_request.clone());
                    }
                    // Packet is an ack
                    PacketType::Ack(ack)=>{
                        self.on_ack_arrived(ack.clone(), packet.clone());
                    }
                    // Packet is a nack
                    PacketType::Nack(nack)=>{
                        self.on_nack_arrived(nack.clone(), packet.clone());
                    } 
                }
            }
            // If ther is an error in reception print error
            Err(err) => {
                self.log(format!(
                    "Server {}: Error receiving packet: {:?}\n", self.server_id, err).as_str(), LogLevel::ERROR);
            }
        }
    }

    /// Handles data requests coming from a client
    fn process_request(&mut self, source_id: NodeId, session_id: u64, raw_content: String, route:Vec<u8>) {
        // Deserialize the contents of the request
        match BrowserRequestWrapper::from_string(raw_content) {
            // If it's ok handle it
            Ok(request_wrapper) => {
                // Check type of the request
                match request_wrapper {
                    BrowserRequestWrapper::Chat(request)=>{
                        match request {
                            // Request asks for the files list
                            BrowserRequest::FileList => self.handle_files_list(source_id, session_id, route),
                            // Request asks for a text file content
                            BrowserRequest::TextFileRequest(id) => {
                                // Check if it's a text server and process the request
                                match self.server_type {
                                    ServerType::Text=>{
                                        self.handle_file_request(id, source_id, session_id, route)
                                    }
                                    // If it's a media server print error
                                    _=>{
                                        self.log(format!("This server cannot handle text file requests\n").as_str(),LogLevel::ERROR);
                                    }
                                }
                            }
                            // Request asks for a media file content
                            BrowserRequest::MediaFileRequest(id) => {
                                // Check if it's a media server and process the request
                                match self.server_type {
                                    ServerType::Media=>{
                                        self.handle_media_request(id, source_id, session_id, route)
                                    }
                                    // If it's a text server print error
                                    _=>{
                                        self.log(format!("This server cannot handle media file requests\n").as_str(),LogLevel::ERROR);
                                    }
                                }
                            }
                        } 
                    }
                    // Request asks for server type
                    BrowserRequestWrapper::ServerType(_request)=>{
                        self.handle_type_request(source_id, session_id, route)
                    }
                }    
            }
            // If's there is an error print it
            Err(err) => {
                self.log(format!("Error deserializing request: {}\n", err).as_str(),LogLevel::ERROR);
            }
        }
    }
    
    /// Send a list of the server file IDs
    pub fn handle_files_list(&mut self, source_id: NodeId, session_id: u64, route:Vec<u8>){
        self.log(format!("Client {} requested file list from server {} of type {:?}\n", source_id, self.server_id, self.server_type).as_str(),LogLevel::INFO);
        //Take file IDs from hashmap
        let mut file_ids=Vec::new();
        match self.server_type {
            ServerType::Text=>{
                file_ids=self.files.keys().cloned().collect();
            }
            ServerType::Media=>{
                file_ids=self.media.keys().cloned().collect();
            }
            ServerType::Chat=>{
                self.log(format!("Error: ServerType::Chat is not supported!\n").as_str(), LogLevel::ERROR);
                std::process::exit(1);
            }
        }
        
        // Create a response with file IDs
        let request=BrowserResponseWrapper::Chat(BrowserResponse::FileList(file_ids));
        // Serialize the response
        let request_json=request.stringify();
        // Send message to client
        self.send_message(source_id, request_json, session_id, route );
    }

    /// Returns a text file based on the id
    pub fn handle_file_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        self.log(format!("Client {} requested a text file from server {}\n", source_id, self.server_id).as_str(),LogLevel::INFO);
        // Search file with that id
        if let Some(file_path)=self.files.get(&id){
            // Read the contents of the file
            match fs::read(file_path) {
                // Convert the content into a string
                Ok(file_data)=>{
                    let file_string = match String::from_utf8(file_data) {
                        Ok(string) => string,
                        Err(err) => {
                            self.log(format!("Error converting file data to String: {}\n", err).as_str(),LogLevel::ERROR);
                            return; 
                        }
                    };
                    // Create a response with text string
                    let request=BrowserResponseWrapper::Chat(BrowserResponse::TextFile(id, file_string));
                    // Serialize the response
                    let request_json=request.stringify();
                    // Send message to client
                    self.send_message(source_id, request_json, session_id, route);
                }
                Err(e)=>{
                    self.log(format!("Error reading file '{}': {}\n", file_path, e).as_str(), LogLevel::ERROR);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            self.log(format!("File with ID '{}' not found\n", id).as_str(),LogLevel::ERROR);
        }
    }

   /// Returns a media file based on the id
    pub fn handle_media_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        self.log(format!("Client {} requested a media file from server {}\n", source_id, self.server_id).as_str(), LogLevel::INFO);
        // Search file with that id
        if let Some(media_path)=self.media.get(&id){
            // Open the image
            match image::open(media_path) {
                Ok(image)=>{
                    // Write image into a vec buffer
                    let mut buffer = Vec::new();
                    match image.write_to(&mut Cursor::new(&mut buffer), ImageFormat::Jpeg) {
                        Ok(_) => {
                            // Create a response with image vec
                            let request = BrowserResponseWrapper::Chat(BrowserResponse::MediaFile(id, buffer));
                            // Serialize the response                            
                            let request_json = request.stringify();
                            // Send message to client
                            self.send_message(source_id, request_json, session_id, route);
                        }
                        Err(e) => {
                            self.log(format!("Error in image: {}\n", e).as_str(), LogLevel::ERROR);
                        }
                    }
                }
                Err(e)=>{
                    self.log(format!("Error reading media '{}': {}\n", media_path, e).as_str(), LogLevel::ERROR);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            self.log(format!("Media with ID '{}' not found\n", id).as_str(),LogLevel::ERROR);
        }
    }

    /// Returns the server type
    pub fn handle_type_request(&mut self, source_id:NodeId, session_id:u64, route:Vec<u8>) {
        self.log(format!("Client {} requested server type from server {}\n", source_id, self.server_id).as_str(),LogLevel::INFO);
        // Create a response with server type
        let request=BrowserResponseWrapper::ServerType(ServerTypeResponse::ServerType(self.server_type.clone()));
        // Serialize the response
        let request_json=request.stringify();
        // Send message to client
        self.send_message(source_id, request_json, session_id, route);
    }

    /// Send a message to a client dividing it into packets using desassembler
    fn send_message(&mut self, destination_id: u8, message: String, session_id: u64, route: Vec<u8>) {
        self.log(format!("Server {} sending message to {}\n", self.server_id, destination_id).as_str(),LogLevel::INFO);
        // Disassemble the message into fragments using deassembler
        let fragments = self
            .deassembler
            .disassemble_message(message.as_bytes().to_vec(), session_id);
        
        // Loop for every fragment generated
        for fragment in fragments {
            // Create a fragment with the fragment ID
            let packet = Packet {
                pack_type: PacketType::MsgFragment(fragment),
                session_id,
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: route.iter().rev().cloned().collect(),
                },
            };
            // Insert the packet into sent_packets
            self.sent_packets.entry(packet.session_id).or_insert_with(Vec::new).push(packet.clone());
            let drone_id = packet.routing_header.hops[1];
            // Send the package to the designated drone
            match self.senders.get(&drone_id) {
                Some(sender) => {
                    sender.send(packet.clone()).unwrap();
                    // Notify the controller that the packet has been sent
                    let _res=self.sim_controller_sender.send(SimControllerResponseWrapper::Event(
                        SimControllerEvent::PacketSent { session_id: packet.session_id.clone(), packet_type: packet.pack_type.to_string() }
                    ));
                }
                // If there is no sender print error
                None => {
                    self.log(format!(
                        "Server {}: No sender found for client {}\n", self.server_id, drone_id
                    ).as_str(),LogLevel::ERROR);
                }
            }
        }
    }

    /// When an ack arrives for a sent packet the corresponding packet is removed from sent_packets
    fn on_ack_arrived(&mut self, ack: Ack, packet:Packet) {
        self.log(format!("Server {} received ACK corresponding to fragment {}\n",self.server_id,  ack.fragment_index).as_str(),LogLevel::DEBUG);

        if let Some(fragments)=self.sent_packets.get_mut(&packet.session_id){
            fragments.retain(|packet|{
                match &packet.pack_type{
                        PacketType::MsgFragment(fragment)=>{
                            fragment.fragment_index!=ack.fragment_index
                        }
                        _=> unreachable!()
                    }
                });


                if fragments.is_empty(){
                    self.sent_packets.remove(&packet.session_id);
                }
            }
        }   

    /// When a nack arrives the type is checked and the corresponding actions are performed and the packet is resent
    fn on_nack_arrived(&mut self, nack: Nack, packet: Packet) {
        self.log(format!("Server {} received NACK corresponding to fragment {}\n",self.server_id,  nack.fragment_index).as_str(),LogLevel::DEBUG);
        // Match nack type
        match nack.nack_type {
            // Resend packet on the same route
            NackType::Dropped=>{
                self.resend_packet(nack.fragment_index, packet.session_id, &mut Vec::new(), nack.nack_type);
            }
            // Need to remove the node and find a new path
            NackType::ErrorInRouting(node_id)=>{
                // Push the packet in nack_queue
                self.nack_queue.entry(packet.session_id).or_insert_with(Vec::new).push(nack);
                // Discover new path
                self.topology.remove_node(node_id);
                self.send_flood_request();
            }
            // Need to find a new path
            _=>{
                // Push the packet in nack_queue
                self.nack_queue.entry(packet.session_id).or_insert_with(Vec::new).push(nack);
                // Discover new path
                self.send_flood_request();
            }
        }
    }

    // Processes all NACKs in the queue and resends the corresponding packets
    pub fn resend_nacks_in_queue(&mut self) {
        self.log(format!("Resending packet for nack\n").as_str(),LogLevel::DEBUG);
        // A collection to store session_ids to remove after processing
        let mut sessions_to_remove = Vec::new();
    
        // Collect all NACKs to resend
        let mut resend_info = Vec::new(); 
    
        // Iterate through each session_id and associated list of NACKs in the queue
        for (session_id, nacks) in self.nack_queue.iter_mut() {
            // Process each NACK in the list and collect the information to resend
            while let Some(nack) = nacks.pop() {
                resend_info.push((nack.fragment_index, *session_id, nack.nack_type));
            }
    
            // If the NACK list is empty after processing, mark the session for removal
            if nacks.is_empty() {
                sessions_to_remove.push(*session_id);
            }
        }
    
        // Now resend the packets collected earlier
        for (fragment_index, session_id, nack_type) in resend_info {
            self.resend_packet(fragment_index, session_id, &mut sessions_to_remove, nack_type);
        }
    
        // Remove the sessions that have no more NACKs to process
        for session_id in sessions_to_remove {
            self.nack_queue.remove(&session_id);
        }
    }
    

    /// Searches for a packet corresponding to a fragment index, resends it by computing the new topology
    fn resend_packet(&mut self, fragment_index: u64, session_id:u64, session_to_remove: &mut Vec<u64>, nack_type: NackType) {

        self.log(format!("Resending packet index={:?}, session={:?}\n",fragment_index,session_id).as_str(),LogLevel::DEBUG);

        if let Some(packets) = self.sent_packets.get_mut(&session_id) {
            if let Some(packet) =packets.iter_mut().find(|p| match &p.pack_type {
                PacketType::MsgFragment(fragment)=>{
                    fragment.fragment_index==fragment_index
                }
                _=>false,
            })  {
                println!("Found packet with session ID: {} and fragment index: {}\n", session_id, fragment_index);
                let destination_id=packet.routing_header.hops[packet.routing_header.hops.len() - 1];
                let new_routing=compute_route(&mut self.topology, self.server_id, destination_id);
                packet.routing_header.hops=new_routing;

                println!("Routing header {:?}\n", packet.routing_header);
                    if packet.routing_header.is_empty() {
                        error!("Route non found\n");
                        self.nack_queue
                            .entry(session_id)
                            .or_insert_with(Vec::new)
                            .push(Nack { fragment_index, nack_type});

                    if let Some(pos) = session_to_remove.iter().position(|&id| id == session_id) {
                        session_to_remove.remove(pos);
                    }
                }
                // Send the packet with the right drone
                let drone_id = packet.routing_header.hops[1];
                match self.senders.get(&drone_id) {
                    Some(sender) => {
                        sender.send(packet.clone()).unwrap_or_else(|err| {
                            error!("Failed to resend packet: {}\n", err);
                        });
                        // Notify the controller about the packet sent
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
                        error!(
                            "Server {}: No sender found for client {}\n", self.server_id, drone_id
                        );
                    
                    }
                }
        } else {
            // If no matching packet is found print error
            error!(
                "No matching packet found for Nack with fragment index: {}\n",
                fragment_index
            );
            }
        } else {
            error!("No packets found for session ID: {}\n", session_id);
        }

    }

    /// If a flood request arrives it adds itself and sends it to the neighbors from which it did not arrive
    fn on_flood_request(&mut self, packet: Packet, mut request: FloodRequest) {
        self.log(format!("Server {} received floodrequest for {:?}\n", self.server_id, request).as_str(),LogLevel::DEBUG);
        // Extract the sender ID
        let sender_id = request.path_trace.last().unwrap().0;
        // Add itself to the request
        request.increment(self.server_id, NodeType::Server);
        // Create a new foold request packet
        let response = Packet::new_flood_request(
            SourceRoutingHeader::empty_route(),
            packet.session_id,
            request,
        );
        // Send the flood request to all neighbors except the sender
        for (neighbor_id, sender) in self.senders.clone() {
            if neighbor_id != sender_id {
                sender.send(response.clone()).unwrap();
            }
        }
    }

    /// When a flood response arrives, it checks the route and adds the corresponding nodes to the topology
    fn on_flood_response(&mut self, flood_response: FloodResponse, packet: Packet) {
        self.log(format!("Server {} received floodresponse for {:?}\n", self.server_id, flood_response).as_str(),LogLevel::DEBUG);


        //Check if the flood response is intended for me
        if flood_response.path_trace.first().map(|node| node.0)!=Some(self.server_id){
            if packet.routing_header.hop_index+1<packet.routing_header.hops.len(){
                let next_hop=packet.routing_header.hops[packet.routing_header.hop_index+1];
                self.log(format!("Server {} forwarding floodresponse to {:?}\n", self.server_id, next_hop).as_str(),LogLevel::DEBUG);


                let forward_packet=Packet{
                    pack_type: PacketType::FloodResponse(flood_response.clone()),
                    session_id: packet.session_id,
                    routing_header: SourceRoutingHeader{
                        hop_index: packet.routing_header.hop_index+1,
                        hops: packet.routing_header.hops.clone(),
                    }
                };
    
                match self.senders.get(&next_hop) {
                    Some(sender) => {
                        sender.send(forward_packet).unwrap_or_else(|err| {
                            self.log(format!("Failed to forward packet: {}\n", err).as_str(),LogLevel::ERROR);
                        });
                    }
                    None => {
                        self.log(format!(
                            "Server {}: No sender found for drone {}\n", self.server_id, next_hop
                        ).as_str(),LogLevel::ERROR);
                    
                    }
                }
            };

            return
        }

        // Iterate through each node in path_trace
        for (i, node) in flood_response.path_trace.iter().enumerate() {
            // If it's not already in the topology add it
            if !self.topology.nodes().contains(&node.0) {
                self.topology.add_node(node.0);
            }
            // For all nodes  check if an edge already exists and if not add it
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
        // Notify the controller indicating that the flood response has been received
        let _res = self
            .sim_controller_sender
            .send(SimControllerResponseWrapper::Message(
                SimControllerMessage::FloodResponse(flood_response.flood_id),
            ));
    }

    /// Send a flood request to neighbors
    pub fn send_flood_request(&mut self) {
        let now = Utc::now().timestamp_millis() as u128;
        let timeout = TIMEOUT_BETWEEN_FLOODS_MS as u128;

        if self.flood_time + timeout >now{
            self.log(format!("Server {} block flood request for timeout\n", self.server_id).as_str(),LogLevel::DEBUG);
            return
        }
        self.flood_time=now;
        self.log(format!("Server {} send flood request\n", self.server_id).as_str(),LogLevel::INFO);
        // Loop through all senders and send a flood request to each one
        for sender in &self.senders {
            let packet = Packet {
                pack_type: PacketType::FloodRequest(FloodRequest {
                    initiator_id: self.server_id,
                    flood_id: rand::random(),
                    path_trace: vec![(self.server_id, NodeType::Server)],
                }),
                session_id: rand::random(),
                routing_header: SourceRoutingHeader {
                    hop_index: 1,
                    hops: Vec::new(),
                },
            };
            sender.1.send(packet).unwrap();
        }
        // Notify the controller indicating that the flood request has been sent
        /*self.sim_controller_sender
            .send(SimControllerResponseWrapper::Event(
                SimControllerEvent::FloodRequestSent,
            ))
            .unwrap();*/
    }

    /// Sends an ack with confirmations to a received packet
    fn send_ack(&mut self, fragment_index:u64, session_id: u64, routing_header: Vec<u8>) {
        self.log(format!("Server {} send ACK from fragment {}\n", self.server_id, fragment_index).as_str(),LogLevel::DEBUG);
        // Create an ACK packet for a specific fragment
        let packet=Packet{
            // fragmnet_index=fragmnet index of the packet
            pack_type:PacketType::Ack(Ack { fragment_index }),
            session_id:session_id,
            routing_header:SourceRoutingHeader {
                hop_index: 1,
                hops: routing_header.iter().rev().cloned().collect(),
            },
        };

        // Send the ack to the right drone
        let drone_id = packet.routing_header.hops[1];
            match self.senders.get(&drone_id) {
                Some(sender) => {
                    sender.send(packet).unwrap();
                }
                None => {
                    // If the drone is not found print error
                    self.log(format!(
                        "Server {}: No sender found for client {}\n",
                        self.server_id,
                        drone_id
                    ).as_str(),LogLevel::ERROR);
                }
            }
    }


    fn log(&self, log_message: &str, log_level: LogLevel) {

        match log_level {
            LogLevel::INFO => {
                print!("LEVEL: INFO >>> {}\n", log_message);
            }
            LogLevel::DEBUG => {
                if self.is_debug {
                    print!("LEVEL: DEBUG >>> {}\n", log_message);
                }
            }
            LogLevel::ERROR => {
                eprint!("LEVEL: ERROR >>> {}\n", log_message);
            }
        }
    }
}







