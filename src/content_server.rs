use std::collections::HashMap;
use std::io::Cursor;
use std::{env, fs};
use image::ImageFormat;
use rustafarian_shared::assembler::{assembler::Assembler, disassembler::Disassembler};
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
    files:HashMap<u8, String>,
    media:HashMap<u8, String>,
    server_type: ServerType,
    nack_queue: HashMap<u64, Vec<Nack>>,
    flooding_in_action: bool
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
        server_type: ServerType
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
                    eprintln!("Error: File directory '{}' does not exist!", file_path.display());
                    std::process::exit(1); 
                }
                // Reads files from directory and places them in files hashmap
                if let Ok(entries) = fs::read_dir(file_path) {
                    for (id, entry) in entries.filter_map(Result::ok).enumerate() {
                        if let Some(path) = entry.path().to_str() {
                            files.insert(id as u8, path.to_string());
                        }
                    }
                }
        
            }
            // If it's a media server upload media files
            ServerType::Media=>{
                let media_path = current_dir.join(media_directory);
                // Check if the directory exist
                if !media_path.exists() {
                    eprintln!("Error: Media directory '{}' does not exist!", media_path.display());
                    std::process::exit(1); 
                }
                // Reads files from directory and places them in media hashmap
                if let Ok(entries) = fs::read_dir(media_path) {
                    for (id, entry) in entries.filter_map(Result::ok).enumerate() {
                        if let Some(path) = entry.path().to_str() {
                            media.insert(id as u8, path.to_string());
                        }
                    }
                }
            }
            // If it's a chat server gives error
            ServerType::Chat=>{
                eprintln!("Error: ServerType::Chat is not supported!");
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
            flooding_in_action: false
        }
    }

    /// Keeps the server active and continuously listens to two main channels
    pub fn run(&mut self) {

        println!("Server {} is running", self.server_id);
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
                        println!("Server {} received addsender", self.server_id);
                        self.senders.insert(id, channel);
                        self.topology.add_node(id);
                        self.topology.add_edge(self.server_id, id);
                    }
                    // Remove a drone from senders
                    SimControllerCommand::RemoveSender(id)=>{
                        println!("Server {} received removesender", self.server_id);
                        self.senders.remove(&id);
                        self.topology.remove_node(id);
                    }    
                    // Other commands are not for this type of server
                    _=>{
                        println!("Client commands")
                    }
                }
            },
            // If there is an error print error
            Err(err) => {
                eprintln!(
                    "Server {}: Error receiving packet from the simulation controller: {:?}",
                    self.server_id,
                    err
                );
            }
        };
    }

    /// Receive packets from the drones channels and handle them
    pub fn handle_drone_packets(&mut self, packet: Result<Packet, crossbeam_channel::RecvError>) {
        match packet {
            // If packet is valid match its type
            Ok(packet) => {
                match &packet.pack_type {
                    // Packet is a message fragment
                    PacketType::MsgFragment(fragment) => {
                        println!("Server {} received fragment {}", self.server_id, packet.get_fragment_index());
                        // Sends ACK that packet has been received
                        self.send_ack(fragment.fragment_index, packet.session_id, packet.routing_header.source().expect("Missing source ID in routing header"));
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
                        self.on_flood_response(flood_response.clone());
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
                eprintln!(
                    "Server {}: Error receiving packet: {:?}", self.server_id, err);
            }
        }
    }

    /// Handles data requests coming from a client
    fn process_request(&mut self, source_id: NodeId, session_id: u64, raw_content: String, route:Vec<u8>) {
        println!("Server {} received complete message from client {}", self.server_id, source_id);
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
                                        eprintln!("This server cannot handle text file requests");
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
                                        eprintln!("This server cannot handle media file requests");
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
                eprintln!("Error deserializing request: {}", err);
            }
        }
    }
    
    /// Send a list of the server file IDs
    pub fn handle_files_list(&mut self, source_id: NodeId, session_id: u64, route:Vec<u8>){
        println!("Client {} requested file list from server {}", source_id, self.server_id);
        //Take file IDs from hashmap
        let file_ids: Vec<u8> = self.files.keys().cloned().collect();
        // Create a response with file IDs
        let request=BrowserResponseWrapper::Chat(BrowserResponse::FileList(file_ids));
        // Serialize the response
        let request_json=request.stringify();
        // Send message to client
        self.send_message(source_id, request_json, session_id, route );
    }

    /// Returns a text file based on the id
    pub fn handle_file_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        println!("Client {} requested a text file from server {}", source_id, self.server_id);
        // Search file with that id
        if let Some(file_path)=self.files.get(&id){
            // Read the contents of the file
            match fs::read(file_path) {
                // Convert the content into a string
                Ok(file_data)=>{
                    let file_string = match String::from_utf8(file_data) {
                        Ok(string) => string,
                        Err(err) => {
                            eprintln!("Error converting file data to String: {}", err);
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
                    eprintln!("Error reading file '{}': {}", file_path, e);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            eprintln!("File with ID '{}' not found", id);
        }
    }

   /// Returns a media file based on the id
    pub fn handle_media_request(&mut self, id:u8, source_id: NodeId, session_id: u64, route:Vec<u8>) {
        println!("Client {} requested a media file from server {}", source_id, self.server_id);
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
                            eprintln!("Error in image: {}", e);
                        }
                    }
                }
                Err(e)=>{
                    eprintln!("Error reading media '{}': {}", media_path, e);
                }
            }
        } else {
            // If the file with that ID does not exist print error
            eprintln!("Media with ID '{}' not found", id);
        }
    }

    /// Returns the server type
    pub fn handle_type_request(&mut self, source_id:NodeId, session_id:u64, route:Vec<u8>) {
        println!("Client {} requested server type from server {}", source_id, self.server_id);
        // Create a response with server type
        let request=BrowserResponseWrapper::ServerType(ServerTypeResponse::ServerType(self.server_type.clone()));
        // Serialize the response
        let request_json=request.stringify();
        // Send message to client
        self.send_message(source_id, request_json, session_id, route);
    }

    /// Send a message to a client dividing it into packets using desassembler
    fn send_message(&mut self, destination_id: u8, message: String, session_id: u64, route: Vec<u8>) {
        println!("Server {} sending message to {}", self.server_id, destination_id);
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
                    eprintln!(
                        "Server {}: No sender found for client {}", self.server_id, drone_id
                    );
                }
            }
        }
    }

    /// When an ack arrives for a sent packet the corresponding packet is removed from sent_packets
    fn on_ack_arrived(&mut self, ack: Ack, packet:Packet) {
        println!("Server {} received ACK corresponding to fragment {}",self.server_id,  ack.fragment_index);

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
        println!("Server {} received NACK corresponding to fragment {}",self.server_id,  nack.fragment_index);
        // Match nack type
        match nack.nack_type {
            // Resend packet on the same route
            NackType::Dropped=>{
                self.resend_packet(nack.fragment_index, packet.session_id);
            }
            // Need to remove the node and find a new path
            NackType::ErrorInRouting(node_id)=>{
                // Push the packet in nack_queue
                self.nack_queue.entry(packet.session_id).or_insert_with(Vec::new).push(nack);
                // Discover new path
                self.topology.remove_node(node_id);
                if self.flooding_in_action==false{
                    self.send_flood_request();
                }
            }
            // Need to find a new path
            _=>{
                // Push the packet in nack_queue
                self.nack_queue.entry(packet.session_id).or_insert_with(Vec::new).push(nack);
                // Discover new path
                if self.flooding_in_action==false{
                    self.send_flood_request();
                }
            }
        }
    }

    // Processes all NACKs in the queue and resends the corresponding packets
    fn resend_nacks_in_queue(&mut self) {
        // A collection to store session_ids to remove after processing
        let mut sessions_to_remove = Vec::new();
    
        // Collect all NACKs to resend
        let mut resend_info = Vec::new();  // Store the necessary data to resend the packets
    
        // Iterate through each session_id and associated list of NACKs in the queue
        for (session_id, nacks) in self.nack_queue.iter_mut() {
            // Process each NACK in the list and collect the information to resend
            while let Some(nack) = nacks.pop() {
                resend_info.push((nack.fragment_index, *session_id));
            }
    
            // If the NACK list is empty after processing, mark the session for removal
            if nacks.is_empty() {
                sessions_to_remove.push(*session_id);
            }
        }
    
        // Now resend the packets collected earlier
        for (fragment_index, session_id) in resend_info {
            self.resend_packet(fragment_index, session_id);
        }
    
        // Remove the sessions that have no more NACKs to process
        for session_id in sessions_to_remove {
            self.nack_queue.remove(&session_id);
        }
    }
    

    /// Searches for a packet corresponding to a fragment index, resends it by computing the new topology
    fn resend_packet(&mut self, fragment_index: u64, session_id:u64) {



        if let Some(packets) = self.sent_packets.get_mut(&session_id) {
            if let Some(packet) =packets.iter_mut().find(|p| match &p.pack_type {
                PacketType::MsgFragment(fragment)=>{
                    fragment.fragment_index==fragment_index
                }
                _=>false,
            })  {
                println!("Found packet with session ID: {} and fragment index: {}", session_id, fragment_index);
                let destination_id=packet.routing_header.hops[packet.routing_header.hops.len() - 1];
                let new_routing=compute_route(&mut self.topology, self.server_id, destination_id);
                packet.routing_header.hops=new_routing;

                // Send the packet with the right drone
                let drone_id = packet.routing_header.hops[1];
                match self.senders.get(&drone_id) {
                    Some(sender) => {
                        sender.send(packet.clone()).unwrap_or_else(|err| {
                            eprintln!("Failed to resend packet: {}", err);
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
                        eprintln!(
                            "Server {}: No sender found for client {}", self.server_id, drone_id
                        );
                    
                }
            }
        } else {
            // If no matching packet is found print error
            eprintln!(
                "No matching packet found for Nack with fragment index: {}",
                fragment_index
            );
            }
        } else {
            // Nessun pacchetto trovato per il session_id specificato
            eprintln!("No packets found for session ID: {}", session_id);
        }

    }


        
                
                

    /// If a flood request arrives it adds itself and sends it to the neighbors from which it did not arrive
    fn on_flood_request(&mut self, packet: Packet, mut request: FloodRequest) {
        println!("Server {} received floodrequest for {:?}", self.server_id, request);
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
    fn on_flood_response(&mut self, flood_response: FloodResponse) {
        println!("Server {} received floodresponse for {:?}", self.server_id, flood_response);
        self.flooding_in_action=false;
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
    fn send_flood_request(&mut self) {
        println!("Server {} send flood request", self.server_id);
        self.flooding_in_action=true;
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
    fn send_ack(&mut self, fragment_index:u64, session_id: u64, destination_id: u8) {
        println!("Server {} send ACK from fragment {}", self.server_id, fragment_index);
        // Create an ACK packet for a specific fragment
        let packet=Packet{
            // fragmnet_index=fragmnet index of the packet
            pack_type:PacketType::Ack(Ack { fragment_index }),
            session_id:session_id,
            routing_header:SourceRoutingHeader {
                hop_index: 1,
                hops: compute_route(&mut self.topology, self.server_id, destination_id),
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
                    eprintln!(
                        "Server {}: No sender found for client {}",
                        self.server_id,
                        drone_id
                    );
                }
            }
    }
}




