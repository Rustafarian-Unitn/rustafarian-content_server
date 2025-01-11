use std::collections::{HashMap, HashSet};
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
use rustafarian_shared::logger::LogLevel::{ERROR,DEBUG,INFO};
use rustafarian_shared::logger::Logger;
use rustafarian_shared::messages::general_messages::{DroneSend, ServerType, ServerTypeResponse};
use rustafarian_shared::topology::{compute_route_dijkstra, Topology};
use wg_2024::packet::{Ack, Nack, NackType, NodeType};
use wg_2024::{
    network::*,
    packet::{FloodRequest, FloodResponse, Packet, PacketType},
};

use crossbeam_channel::{select_biased, Receiver, Sender};


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
    pub packet_to_retry: HashSet<(u64,u64)>,
    flood_time: u128,
    is_debug: bool,
    logger:Logger,
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
            flood_time:0,
            is_debug,
            logger:Logger::new("Content Server".to_string(), server_id, is_debug),
            packet_to_retry:HashSet::new(),
        }
    }

    /// Keeps the server active and continuously listens to two main channels
    pub fn run(&mut self) {

        self.logger.log(
            format!("Server {} is running\n", self.server_id).as_str(),INFO);
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
                        self.logger.log(format!("Server {} received addsender\n", self.server_id).as_str(), INFO);
                        self.handle_add_sender(id, channel);
                    }
                    // Remove a drone from senders
                    SimControllerCommand::RemoveSender(id)=>{
                        self.logger.log(format!("Server {} received removesender\n", self.server_id).as_str(),INFO);
                        self.handle_remove_sender(id);
                    }   
                    // Send server topology
                    SimControllerCommand::Topology=>{
                        self.handle_topology_request();
                    } 
                    // Other commands are not for this type of server
                    _=>{
                        self.logger.log(format!("Client commands\n").as_str(),ERROR);
                    }
                }
            },
            // If there is an error print error
            Err(err) => {
                self.logger.log(format!(
                    "Server {}: Error receiving packet from the simulation controller: {:?}\n",
                    self.server_id,
                    err).as_str(),ERROR
                );
            }
        };
    }


    fn handle_add_sender(&mut self, id: NodeId, channel: Sender<Packet>) {
        self.senders.insert(id, channel);
        self.topology.add_node(id);
        self.topology.add_edge(self.server_id, id);
        self.send_flood_request();
    }

    fn handle_remove_sender(&mut self, id: NodeId) {
        self.senders.remove(&id);
        self.topology.remove_edges(self.server_id, id);
    }

    fn handle_topology_request(&mut self) {
        self.logger.log(format!("Server {} received topology request\n", self.server_id).as_str(),INFO);

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
                        self.logger.log(format!("Server {} received fragment {}\n", self.server_id, packet.get_fragment_index()).as_str(),DEBUG);
                        // Sends ACK that packet has been received
                        self.send_ack(fragment.fragment_index, packet.session_id, packet.routing_header.hops.clone());
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
                self.logger.log(format!(
                    "Server {}: Error receiving packet: {:?}\n", self.server_id, err).as_str(), ERROR);
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
                                        self.logger.log(format!("This server cannot handle text file requests\n").as_str(),ERROR);
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
                                        self.logger.log(format!("This server cannot handle media file requests\n").as_str(),ERROR);
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
                self.logger.log(format!("Error deserializing request: {}\n", err).as_str(),ERROR);
            }
        }
    }
    
    /// Send a list of the server file IDs with a FileList message matching the server type
    pub fn handle_files_list(&mut self, source_id: NodeId, session_id: u64, route:Vec<u8>){
        self.logger.log(format!("Client {} requested file list from server {} of type {:?}\n", source_id, self.server_id, self.server_type).as_str(),INFO);
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
                self.logger.log(format!("Error: ServerType::Chat is not supported!\n").as_str(), ERROR);
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

    /// Returns a text file based on the id with a TextFile message
    pub fn handle_file_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        self.logger.log(format!("Client {} requested a text file from server {}\n", source_id, self.server_id).as_str(),INFO);
        // Search file with that id
        if let Some(file_path)=self.files.get(&id){
            // Read the contents of the file
            match fs::read(file_path) {
                // Convert the content into a string
                Ok(file_data)=>{
                    let file_string = match String::from_utf8(file_data) {
                        Ok(string) => string,
                        Err(err) => {
                            self.logger.log(format!("Error converting file data to String: {}\n", err).as_str(),ERROR);
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
                    self.logger.log(format!("Error reading file '{}': {}\n", file_path, e).as_str(), ERROR);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            self.logger.log(format!("File with ID '{}' not found\n", id).as_str(),ERROR);
        }
    }

   /// Returns a media file based on the id with a MediaFile message
    pub fn handle_media_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        self.logger.log(format!("Client {} requested a media file from server {}\n", source_id, self.server_id).as_str(), INFO);
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
                            self.logger.log(format!("Error in image: {}\n", e).as_str(), ERROR);
                        }
                    }
                }
                Err(e)=>{
                    self.logger.log(format!("Error reading media '{}': {}\n", media_path, e).as_str(), ERROR);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            self.logger.log(format!("Media with ID '{}' not found\n", id).as_str(),ERROR);
        }
    }

    /// Returns the server type with a ServerTypeResponse message
    pub fn handle_type_request(&mut self, source_id:NodeId, session_id:u64, route:Vec<u8>) {
        self.logger.log(format!("Client {} requested server type from server {}\n", source_id, self.server_id).as_str(),INFO);
        // Create a response with server type
        let request=BrowserResponseWrapper::ServerType(ServerTypeResponse::ServerType(self.server_type.clone()));
        // Serialize the response
        let request_json=request.stringify();
        // Send message to client
        self.send_message(source_id, request_json, session_id, route);
    }

    /// Disassembles a message into fragments, 
    /// for each fragment it creates a packet and as routing header uses the reverse route of the request, 
    /// after which it puts the packets in the list of sent packets and sends them to the first drone
    fn send_message(&mut self, destination_id: u8, message: String, session_id: u64, route: Vec<u8>) {
        self.logger.log(format!("Server {} sending message to {}\n", self.server_id, destination_id).as_str(),INFO);
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
                }
                // If there is no sender print error
                None => {
                    self.logger.log(format!(
                        "Server {}: No sender found for client {}\n", self.server_id, drone_id
                    ).as_str(),ERROR);
                }
            }
        }
        // Notify the controller that the packet has been sent
        let _res=self.sim_controller_sender.send(SimControllerResponseWrapper::Event(
            SimControllerEvent::MessageSent { session_id: session_id } 
        ));
    }

    /// When an ack arrives for a sent packet the corresponding packet is removed from sent_packets
    fn on_ack_arrived(&mut self, ack: Ack, packet:Packet) {
        self.logger.log(format!("Server {} received ACK corresponding to fragment {}\n",self.server_id,  ack.fragment_index).as_str(),DEBUG);

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

    /// It takes a copy of the packet corresponding to the nack from the list of sent packets, 
    /// if the packet is dropped it sends it back, 
    /// if it is a routing error it also removes the node and starts a flood request
    fn on_nack_arrived(&mut self, nack: Nack, packet: Packet) {
        self.logger.log(format!("Server {} received NACK corresponding to fragment {}\n",self.server_id,  nack.fragment_index).as_str(),DEBUG);
        let sent_packet_clone:Packet;
        let sent_packets_cloned=self.sent_packets.get(&packet.session_id).cloned();
        match sent_packets_cloned {
            Some(sent_packets)=>{
                sent_packet_clone=sent_packets.get(nack.fragment_index as usize).unwrap().clone();
                match nack.nack_type {

                    // Resend packet on the same route
                    NackType::Dropped=>{
                        self.resend_packet(sent_packet_clone);
                    }
                    // Need to remove the node and find a new path
                    NackType::ErrorInRouting(node_id)=>{
                        // Discover new path
                        self.topology.remove_node(node_id);
                        self.send_flood_request();
                        self.resend_packet(sent_packet_clone);
                    }
                    // Need to find a new path
                    _=>{
                        // Discover new path
                        self.send_flood_request();
                        self.resend_packet(sent_packet_clone);
                    }
                }
            },
            None=>{
                self.logger.log(&format!("Packets not found for Nack"), ERROR)
            }
        }
        
        
    }


    /// It takes a packet as input and calculates the route, 
    /// if it doesn't find it it puts it in a waiting queue and sends a flood request 
    /// otherwise it sends it to the first drone
    pub fn resend_packet(&mut self, mut packet: Packet) {
        let new_routing=compute_route_dijkstra(&mut self.topology, self.server_id, packet.routing_header.destination().unwrap());
        packet.routing_header.hops=new_routing;

        let route_to_check=packet.routing_header.hops.clone();

        //If route does not exist put in resend queue
        if route_to_check.is_empty() {
            self.packet_to_retry.insert((packet.session_id,packet.get_fragment_index()));
            self.send_flood_request();
            return
        }
        //send the packet
        let drone_id = packet.routing_header.hops[1];
        match self.senders.get(&drone_id) {
            Some(sender) => {
                sender.send(packet.clone()).unwrap();
                self.packet_to_retry.remove(&(packet.session_id,packet.get_fragment_index()));
            }
            // If there is no sender print error
            None => {
                self.logger.log(format!(
                    "Server {}: No sender found for client {}\n", self.server_id, drone_id
                ).as_str(),ERROR);
            }
        }
    }


    /// If a flood request arrives it adds itself and sends it to the neighbors from which it did not arrive
    fn on_flood_request(&mut self, packet: Packet, mut request: FloodRequest) {
        self.logger.log(format!("Server {} received floodrequest for {:?}\n", self.server_id, request).as_str(),DEBUG);
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

    /// When a flood response arrives it checks whether it was sent by itself, 
    /// if not, it forwards it to the next hop, 
    /// otherwise it updates the topology and tries to send the packets back to the waiting queue
    fn on_flood_response(&mut self, flood_response: FloodResponse, packet: Packet) {
        self.logger.log(format!("Server {} received floodresponse for {:?}\n", self.server_id, flood_response).as_str(),DEBUG);


        //Check if the flood response is intended for me
        if flood_response.path_trace.first().map(|node| node.0)!=Some(self.server_id){
            if packet.routing_header.hop_index+1<packet.routing_header.hops.len(){
                let next_hop=packet.routing_header.hops[packet.routing_header.hop_index+1];
                self.logger.log(format!("Server {} forwarding floodresponse to {:?}\n", self.server_id, next_hop).as_str(),DEBUG);


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
                            self.logger.log(format!("Failed to forward packet: {}\n", err).as_str(),ERROR);
                        });
                    }
                    None => {
                        self.logger.log(format!(
                            "Server {}: No sender found for drone {}\n", self.server_id, next_hop
                        ).as_str(),ERROR);
                    
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


        self.resend_packets_in_queue();
    }

    /// Copies the queue of packets to be resent and resends each packet corresponding to the queue
    fn resend_packets_in_queue(&mut self) {
        let packet_to_retry_clone=self.packet_to_retry.clone();

        for (session_id,fragment_index) in packet_to_retry_clone{
            if let Some(fragments)=self.sent_packets.get(&session_id).cloned(){
                let fragment=fragments.get(fragment_index as usize).unwrap().clone();
                self.resend_packet(fragment);
            }
        }
    }

    /// Send a flood request to neighbors
    pub fn send_flood_request(&mut self) {
        let now = Utc::now().timestamp_millis() as u128;
        let timeout = TIMEOUT_BETWEEN_FLOODS_MS as u128;

        if self.flood_time + timeout >now{
            self.logger.log(format!("Server {} block flood request for timeout\n", self.server_id).as_str(),DEBUG);
            return
        }
        self.flood_time=now;
        self.logger.log(format!("Server {} send flood request\n", self.server_id).as_str(),INFO);
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
        self.sim_controller_sender
            .send(SimControllerResponseWrapper::Event(
                SimControllerEvent::FloodRequestSent,
            ))
            .unwrap();
    }

    /// Sends an ack with confirmations to a received packet
    fn send_ack(&mut self, fragment_index:u64, session_id: u64, routing_header: Vec<u8>) {
        self.logger.log(format!("Server {} send ACK from fragment {}\n", self.server_id, fragment_index).as_str(),DEBUG);
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
                    self.logger.log(format!(
                        "Server {}: No sender found for client {}\n",
                        self.server_id,
                        drone_id
                    ).as_str(),ERROR);
                }
            }
    }


    
}







