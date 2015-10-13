// Copyright 2015 Eluvatar
//
// This file is part of Trawler.
//
// Trawler is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Trawler is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Trawler.  If not, see <http://www.gnu.org/licenses/>.

// command line arguments
extern crate getopts;
use getopts::Options;
use std::env;

// protocol 
// https://github.com/stepancheg/rust-protobuf
// and https://github.com/Eluvatar/trawler-protocol
extern crate protobuf;
mod trawler;
use trawler::Request_Method as Method;
use protobuf::core::Message;

// zmq https://github.com/erickt/rust-zmq
// used
// https://github.com/dginev/rust-zmq/blob/aa28c0656aa2e969bbabbcc48ebad7961a325208/examples/zguide/rtdealer/main.rs
// as reference for ROUTER behavior
extern crate zmq;
use zmq::{SNDMORE};

// instead of curl, hyper https://github.com/hyperium/hyper 
extern crate hyper;
use hyper::Client;
use hyper::header::{UserAgent,Header,Cookie};

// time crate.. TODO Cargo.toml
extern crate time;
use time::*;

// simple std structures

use std::collections::LinkedList;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

// other necessary std imports

use std::clone::Clone;
use std::io::Read;

// constants
const ACK: i32= 200;
const LOGOUT__LOGIN_SYNTAX: i32 = 400;
const LOGOUT__TIMEOUT: i32 = 408;
const LOGOUT__SHUTDOWN: i32 = 503;
#[allow(dead_code)]
const NACK__GENERIC: i32 = 500;
const NACK__UNSUPPORTED_METHOD: i32 = 501;

const BASE_URL: &'static str = "http://localhost:6260/";

const PORT: i32 = 5557;

// TODO reject requests when at max
// const MAX_REQUESTS: i32 = 1000;
const SESSION_TIMEOUT: f64 = 30.0;

const NS_DELAY_MSEC: i64 = 660;

// TODO compose user_agent string using VERSION and client-supplied user_agent component
//const VERSION: &'static str = env!("CARGO_PKG_VERSION");

struct TRequest {
    client: Vec<u8>,
    id: i32,
    method: Method,
    path: String,
    query: String,
    session: Option<Cookie>,
    headers: bool,
}

struct TSession {
    last_activity: f64,
    req_count: u32,
    user_agent: String,
}

struct Trawler {
    #[allow(dead_code)]
    context: zmq::Context,
    sock: zmq::Socket,
    requests: LinkedList<TRequest>,
    sessions: HashMap<Vec<u8>, TSession>,
    http_client: Client,
    interrupted: bool,
}

macro_rules! compose_error {
    ($composed:ident, {$($from:ty => $member:ident),*} ) => {
        #[derive(Debug)]
        enum $composed {
            $( $member($from) ),*
        }
        $(impl From<$from> for $composed {
            fn from(err: $from) -> $composed {
                $composed::$member(err)
            }
        })*
    }
}

compose_error!{ TrawlerError, {
    std::io::Error => IoError,
    protobuf::ProtobufError => ProtobufError,
    zmq::Error => ZmqError,
    hyper::error::Error => HyperError
}}

fn print_usage(program: &str, opts: Options) {
        let brief = format!("Usage: {} [options]", program);
            print!("{}", opts.usage(&brief));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optopt("p", "port", &format!("set listening port (default={})",PORT), "PORT");
    opts.optflag("h", "help", "print this help menu");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => { m }
        Err(f) => {
            println!("{}",f);
            print_usage(&program, opts);
            return;
        }
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }
    let mut trawler = match matches.opt_str("p") {
        None => Trawler::new(),
        Some(port_string) => Trawler::with_port(port_string.parse::<i32>().unwrap()),
    };
    trawler.run();
}

impl Trawler {
    pub fn new() -> Trawler {
        Trawler::with_port(PORT)
    }
    pub fn with_port(port: i32) -> Trawler {
        let mut context = zmq::Context::new();
        let mut sock = context.socket(zmq::ROUTER).unwrap();
        sock.bind(&("tcp://*:".to_string()+&port.to_string())).unwrap();
        Trawler{
            context: context,
            sock: sock,
            requests: LinkedList::new(),
            sessions: HashMap::new(),
            http_client: Client::new(),
            interrupted: false,
        }
    }
    fn poll(&mut self, timeout: Duration) -> Result<i32,zmq::Error> {
        let poll_item = self.sock.as_poll_item(zmq::POLLIN);
        let mut poll_items = [poll_item; 1];
        zmq::poll(&mut poll_items, timeout.num_milliseconds())
    }
    fn run(&mut self) {
        let mut timeout = Duration::milliseconds(NS_DELAY_MSEC);
        let mut then = get_time();
        let mut now = then;
        // TODO confirm events=0 is fine
        // TODO signal handling (not necessarily here)
        loop {
            if self.interrupted {
                return;
            }
            if self.requests.is_empty() {
                self.receive().unwrap_or_else( |err|
                    println!("Error while waiting for connections: {:?}", err)
                );
            } else {
                match self.poll(timeout) {
                    Err(_) => {
                        self.shutdown();
                        panic!("Polling error from zmq!");
                    },
                    Ok(1) => {
                        // TODO check order
                        timeout = Trawler::api_timeout(then, now);
                        self.receive().unwrap_or_else( |err|
                            println!("Error while listening and waiting to make request: {:?}", err)
                        );
                        now = get_time();
                    },
                    Ok(0) => {
                        // TODO check order
                        let active_request = self.requests.pop_front()
                            .expect("no items should be removed since check");
                        then = now;
                        timeout = Duration::milliseconds(NS_DELAY_MSEC);
                        self.fulfill_request( active_request ).unwrap_or_else( |err|
                            println!("Error fulfilling request: {:?}", err)
                        );
                    },
                    Ok(_) => {
                        self.shutdown();
                        panic!("Invalid poll response from zmq!");
                    }
                }

            }
            self.reap();
        }
    }
    fn api_timeout(then: Timespec, now: Timespec) -> Duration {
        Duration::milliseconds(NS_DELAY_MSEC) + (then - now)
    }
    fn reap(&mut self){
        let mut to_remove: LinkedList<Vec<u8>> = LinkedList::new();
        let now = precise_time_s();
        for (client, session) in self.sessions.iter() {
            if session.req_count == 0u32 
                && now > session.last_activity + SESSION_TIMEOUT {
                to_remove.push_back(Clone::clone(client));
            }
        }
        for client in &to_remove {
            self.sessions.remove(client);
            Trawler::logout(&mut self.sock, client, LOGOUT__TIMEOUT).unwrap_or_else( |err| {
                print!("Error while timing out 0x");
                for byte in client {
                    print!("{:02x}", byte);
                }
                println!(": {:?}", err);
            });
        }
    }
    fn receive(&mut self) -> Result<(),TrawlerError> {
        if self.interrupted {
            return Ok(());
        }
        let mut mclient = zmq::Message::new().unwrap();
        self.sock.recv(&mut mclient, 0).unwrap();
        if self.interrupted {
            return Ok(());
        }
        let client: &[u8] = &(*mclient);
        let mut mcontent = zmq::Message::new().unwrap();
        self.sock.recv(&mut mcontent, 0).unwrap();
        let content: &[u8] = &(*mcontent);
        match self.sessions.entry(Clone::clone(&client.to_vec())) {
            Entry::Occupied(mut occupied) => {
                let mut session = occupied.get_mut();
                session.update_last_activity();
            },
            Entry::Vacant(vacant) => {
                return match Trawler::make_login(content) {
                    Ok(login) => {
                        assert!(login.has_user_agent());
                        vacant.insert(TSession::new(&login));
                        Ok(())
                    },
                    Err(_) => Trawler::logout(&mut self.sock, client, LOGOUT__LOGIN_SYNTAX),
                }
            }
        }
        match Trawler::make_request(content) {
            Ok(request) => self.request(client, &request),
            Err(e) => Err(e),
        }
    }
    fn make_login(content: &[u8]) -> Result<trawler::Login,TrawlerError> {
        let mut login = trawler::Login::new();
        try!(login.merge_from_bytes(content));
        Ok(login)
    }
    fn make_request(content: &[u8]) -> Result<trawler::Request,TrawlerError> {
        let mut request = trawler::Request::new();
        try!(request.merge_from_bytes(content));
        Ok(request)
    }
    fn request(&mut self, client: &[u8], preq: &trawler::Request) -> Result<(),TrawlerError> {
        match preq.get_method() {
            Method::GET | Method::HEAD | Method::POST => {
                self.requests.push_back(TRequest::new(client, preq));
                self.ack(client, preq.get_id())
            },
            _ => {
                self.nack(client, preq.get_id(), NACK__UNSUPPORTED_METHOD)
            },
        }
    }
    fn ack(&mut self, client: &[u8], req_id: i32) -> Result<(),TrawlerError> {
        let mut reply = trawler::Reply::new();
        reply.set_reply_type(trawler::Reply_ReplyType::Ack);
        reply.set_req_id(req_id);
        reply.set_result(ACK);
        Trawler::reply(&mut self.sock, client, &reply)
    }
    fn nack(&mut self, client: &[u8], req_id: i32, result: i32) -> Result<(),TrawlerError> {
        if self.interrupted {
            return Ok(());
        }
        let mut reply = trawler::Reply::new();
        reply.set_reply_type(trawler::Reply_ReplyType::Nack);
        reply.set_req_id(req_id);
        reply.set_result(result);
        Trawler::reply(&mut self.sock, client, &reply)
    }
    fn logout(sock: &mut zmq::Socket, client: &[u8], result: i32) -> Result<(),TrawlerError> {
        let mut reply = trawler::Reply::new();
        reply.set_reply_type(trawler::Reply_ReplyType::Logout);
        reply.set_req_id(0);
        reply.set_result(result);
        Trawler::reply(sock, client, &reply)
    }
    fn fulfill_request(&mut self, request: TRequest) -> Result<(),TrawlerError> {
        let mut reply = trawler::Reply::new();
        reply.set_req_id(request.id);
        let mut session = self.sessions.get_mut(&request.client).expect("must have session to fulfill request");
        session.req_count -= 1;
        let mut url = "".to_string();
        let rb = match request.method {
            Method::GET => {
                url = url + BASE_URL + &request.path + "?" + &request.query;
                self.http_client.get(&url)
            },
            Method::HEAD => {
                url = url + BASE_URL + &request.path + "?" + &request.query;
                self.http_client.head(&url)
            },
            Method::POST => {
                url = url + BASE_URL + &request.path;
                self.http_client.post(&url).body(&request.query)
            },
            _ => panic!("Unsupported method invoked, somehow!?"),
        };
        let rb = rb.header(request.session).header(UserAgent(session.user_agent.to_string()));
        let mut result = try!(rb.send());
        reply.set_result(result.status.to_u16() as i32);
        reply.set_continued(true);
        if request.headers {
            for hv in result.headers.iter() {
                reply.set_headers(hv.to_string().into_bytes());
                try!(Trawler::reply(&mut self.sock, &request.client, &reply));
            }
            reply.clear_headers();
        }
        let mut buffer = [0u8; 0x4000];
        let mut len = try!(result.read(&mut buffer[..]));
        while len > 0 {
            reply.set_response(buffer[..len].to_vec());
            try!(Trawler::reply(&mut self.sock, &request.client, &reply));
            len = try!(result.read(&mut buffer[..]));
        }
        reply.clear_response();
        reply.set_continued(false);
        Ok(try!(Trawler::reply(&mut self.sock, &request.client, &reply)))
    }
    fn reply(sock: &mut zmq::Socket, client: &[u8], reply: &trawler::Reply) -> Result<(),TrawlerError> {
        let buffer = try!(reply.write_to_bytes());
        try!(sock.send(client, SNDMORE));
        try!(sock.send(b"", SNDMORE));
        Ok(try!(sock.send(&buffer, 0)))
    }
    fn shutdown(&mut self) {
        for client in self.sessions.keys() {
            Trawler::logout(&mut self.sock, client, LOGOUT__SHUTDOWN).unwrap_or_else( |err| {
                print!("Error while disconnecting 0x");
                for byte in client {
                    print!("{:02x}", byte);
                }
                println!(": {:?}", err);
            });
        }
        self.sessions.clear();
    }
}
impl Drop for Trawler {
    fn drop(&mut self ) {
        self.shutdown();
    }
}

impl TSession {
    fn new( login: &trawler::Login ) -> TSession {
        TSession{
            last_activity: precise_time_s(),
            req_count: 0,
            user_agent: login.get_user_agent().to_string(),
        }
    }
    fn update_last_activity(&mut self){
        self.last_activity = precise_time_s();
    }
}

impl TRequest {
    fn new(client: &[u8], preq: &trawler::Request) -> TRequest {
        TRequest {
            client: client.to_vec(),
            id: preq.get_id(),
            method: preq.get_method(),
            path: preq.get_path().to_string(),    
            query: preq.get_query().to_string(),
            session: if preq.has_session() {
                Some(TRequest::cookie(preq.get_session()))
            } else {
                None
            },
            headers: preq.has_headers() && preq.get_headers(),
        }
    }
    fn cookie(session: &str) -> Cookie {
        let a = session.to_string();
        let b = a.into_bytes();
        Cookie::parse_header(&[b]).unwrap_or_else({ |err|
            panic!("Error while parsing requested cookies (session field): {:?}", err)
        })
    }
}
