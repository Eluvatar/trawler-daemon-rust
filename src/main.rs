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

// rate limits
extern crate regex;
mod throttles;
use throttles::{Throttle,TRequest};

// zmq https://github.com/erickt/rust-zmq
// used
// https://github.com/dginev/rust-zmq/blob/aa28c0656aa2e969bbabbcc48ebad7961a325208/examples/zguide/rtdealer/main.rs
// as reference for ROUTER behavior
extern crate zmq;
use zmq::{SNDMORE};

// instead of curl, hyper https://github.com/hyperium/hyper 
extern crate hyper;
use hyper::Client;
use hyper::header::{UserAgent};
//use hyper::error::Error as HyperError;
use hyper::error::Error::Io as HyperIoError;

// time crate
extern crate time;
use time::*;

// error_type crate
#[macro_use] extern crate error_type;

// simple std structures

use std::collections::LinkedList;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

// other necessary std imports

use std::clone::Clone;
use std::io::Read;
use std::io::ErrorKind::{ConnectionAborted,ConnectionReset};
use std::fmt;

// constants
const ACK: i32= 200;
const LOGOUT__LOGIN_SYNTAX: i32 = 400;
const LOGOUT__TIMEOUT: i32 = 408;
const LOGOUT__SHUTDOWN: i32 = 503;
#[allow(dead_code)]
const NACK__GENERIC: i32 = 500;
const NACK__UNSUPPORTED_METHOD: i32 = 501;

const DEBUG_BASE_URL: &'static str = "http://localhost:6260/";
const HTTP_BASE_URL: &'static str = "http://www.nationstates.net/";
const HTTPS_BASE_URL: &'static str = "https://www.nationstates.net/";

const PORT: i32 = 5557;

// TODO reject requests when at max
// const MAX_REQUESTS: i32 = 1000;
const SESSION_TIMEOUT: f64 = 30.0;

// TODO compose user_agent string using VERSION and client-supplied user_agent component
//const VERSION: &'static str = env!("CARGO_PKG_VERSION");

fn client_hex(client: &[u8]) -> String {
    // TODO uncomment when 1.4.0
    // client.iter().map(|byte| format!("{:02x}", byte)).collect()
    client.iter().map(|byte| format!("{:02x}", byte)).fold(String::new(), |a, b| a+&*b)
}

fn now_ts() -> String {
    let _now = now();
    format!("{}.{:03}", strftime("%Y-%m-%d %T",&_now).unwrap(), _now.tm_nsec/1_000_000)
}

impl fmt::Debug for TRequest {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("TRequest")
            .field("client", &client_hex(&self.client))
            .field("id", &self.id)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("session", &self.session)
            .field("headers", &self.headers)
            .finish()
    }
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
    throttle: Throttle,
    sessions: HashMap<Vec<u8>, TSession>,
    http_client: Client,
    base_url: &'static str,
    interrupted: bool,
    verbose: bool,
}

error_type! {
    #[derive(Debug)]
    pub enum TrawlerError {
        IoError(std::io::Error) { cause; },
        ProtobufError(protobuf::ProtobufError) { cause; },
        ZmqError(zmq::Error) {},
        HyperError(hyper::error::Error) { cause; }
    }
}

fn print_usage(program: &str, opts: Options) {
        let brief = format!("Usage: {} [options]", program);
            print!("{}", opts.usage(&brief));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optopt("p", "port", &format!("set listening port (default={})",PORT), "PORT");
    opts.optflag("d", "debug", "hit mock server instead of nationstates.net");
    opts.optflag("s", "secure", "hit https://www.nationstates.net (overrides --debug)");
    opts.optflag("v", "verbose", "log requests to stdout");
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
    let port = match matches.opt_str("p") {
        None => None, 
        Some(port_string) => match port_string.parse::<i32>() {
            Ok(i) => Some(i),
            Err(_) => panic!("port must be an integer!"),
        },
    };
    let base_url = if matches.opt_present("s") {
        Some(HTTPS_BASE_URL)
    } else if matches.opt_present("d") {
        Some(DEBUG_BASE_URL)
    } else {
        None
    };
    let verbose = if matches.opt_present("v") {
        Some(true)
    } else {
        None
    };
    let mut trawler = Trawler::new(port, base_url, verbose);
    trawler.run();
}

impl Trawler {
    pub fn new(port: Option<i32>, base_url: Option<&'static str>, verbose: Option<bool>) -> Trawler {
        Trawler::with(port.unwrap_or(PORT),
                      base_url.unwrap_or(HTTP_BASE_URL),
                      verbose.unwrap_or(false))
    }
    pub fn with(port: i32, base_url: &'static str, verbose: bool) -> Trawler {
        let mut context = zmq::Context::new();
        let mut sock = context.socket(zmq::ROUTER).unwrap();
        sock.bind(&("tcp://*:".to_string()+&port.to_string())).unwrap();
        Trawler{
            context: context,
            sock: sock,
            throttle: Throttle::nssite(),
            sessions: HashMap::new(),
            http_client: Client::new(),
            base_url: base_url,
            interrupted: false,
            verbose: verbose
        }
    }
    fn poll(&mut self, timeout: Duration) -> Result<i32,zmq::Error> {
        let poll_item = self.sock.as_poll_item(zmq::POLLIN);
        let mut poll_items = [poll_item; 1];
        let tm_millis = timeout.num_milliseconds();
        let to_sleep = if tm_millis > 0 {
            tm_millis
        } else {
            0
        };
        if self.verbose {
            println!("{} polling with timeout of {} ...",
                     now_ts(), to_sleep);
        }
        zmq::poll(&mut poll_items, to_sleep)
    }
    fn run(&mut self) {
        // TODO signal handling (not necessarily here)
        loop {
            if self.interrupted {
                return;
            }
            if self.throttle.is_empty() {
                self.receive().unwrap_or_else( |err|
                    println!("Error while waiting for connections: {:?}", err)
                );
            } else {
                let timeout = self.throttle.timeout();
                let poll_res = self.poll(timeout);
                match poll_res {
                    Err(_) => {
                        self.shutdown();
                        panic!("Polling error from zmq!");
                    },
                    Ok(1) => {
                        self.receive().unwrap_or_else( |err|
                            println!("{} Error while listening and waiting to make request: {:?}",
                                     now_ts(), err)
                        );
                    },
                    Ok(0) => {
                        assert!(! self.throttle.is_empty());
                        self.fulfill_request().unwrap_or_else( |err|
                            println!("{} Error fulfilling request: {:?}",
                                     now_ts(), err)
                        );
                    },
                    Ok(_) => {
                        self.shutdown();
                        panic!("Invalid poll response from zmq!");
                    }
                }
                if self.verbose {
                    println!("{} throttle: {:?}", now_ts(), self.throttle);
                }
            }
            self.reap();
        }
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
                println!("{} Error while timing out {}: {:?}",
                         now_ts(), client_hex(client), err);
            });
            if self.verbose {
                println!("{} Timed out {}", now_ts(), client_hex(client));
            }
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
                        if self.verbose {
                            println!("{} {:?} sent {:?}", now_ts(), client_hex(client), login);
                        }
                        vacant.insert(TSession::new(&login));
                        Ok(())
                    },
                    Err(_) => {
                        println!("{} Sent logout to {}, as they were not logged in",
                                 now_ts(), client_hex(client));
                        Trawler::logout(&mut self.sock, client, LOGOUT__LOGIN_SYNTAX)
                    },
                }
            }
        }
        match Trawler::make_request(content) {
            Ok(request) => {
                if self.verbose {
                    println!("{} {:?} sent {:?}", now_ts(), client_hex(client), request);
                }
                self.request(client, &request)
            },
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
                self.throttle.queue_request(TRequest::new(client, preq));
                match self.sessions.entry(client.to_vec()) {
                    Entry::Occupied(mut occupied) => {
                        let session = occupied.get_mut();
                        session.req_count += 1;
                    },
                    Entry::Vacant(_) => unreachable!()
                }
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
    fn fulfill_request(&mut self) -> Result<(),TrawlerError> {
      let sessions = &mut self.sessions;
      let base_url = self.base_url;
      let http_client = &mut self.http_client;
      let verbose = self.verbose;
      let sock = &mut self.sock;
      self.throttle.span_request(|request| { 
        let mut reply = trawler::Reply::new();
        reply.set_reply_type(trawler::Reply_ReplyType::Response);
        reply.set_req_id(request.id);
        let user_agent: Option<UserAgent>;
        match sessions.get_mut(&request.client) {
            Some(session) => {
                user_agent = Some(UserAgent(session.user_agent.to_string()));
                session.req_count -= 1;
            },
            None => panic!("must have session to fulfill request"),
        }
        let mut res = Ok(());
        for i in 0..8 {
            let mut url = String::new();
            let rb = match request.method {
                Method::GET => {
                    url = url + base_url + &request.path + "?" + &request.query;
                    http_client.get(&url)
                },
                Method::HEAD => {
                    url = url + base_url + &request.path + "?" + &request.query;
                    http_client.head(&url)
                },
                Method::POST => {
                    url = url + base_url + &request.path;
                    http_client.post(&url).body(&request.query)
                },
                _ => panic!("Unsupported method invoked, somehow!?"),
            };
            let rb = match request.session {
                Some(ref session) => rb.header(Clone::clone(session)),
                None => rb,
            }.header(Clone::clone(&user_agent).unwrap());
            let result = rb.send();
            match result {
                Err(HyperIoError(err)) => {
                    if err.kind() == ConnectionAborted
                        || err.kind() == ConnectionReset {
                        println!("{} io error: {:?}", now_ts(), err);
                        res = Err(TrawlerError::from(HyperIoError(err)));
                        std::thread::sleep_ms(128u32 << i);
                    } else {
                        try!(Trawler::fail(sock, &request.client, &mut reply));
                        return Err(TrawlerError::from(err));
                    }
                },
                Err(err) => {
                    try!(Trawler::fail(sock, &request.client, &mut reply));
                    return Err(TrawlerError::from(err));
                },
                Ok(mut response) => {
                    if verbose {
                        println!("{} {:?} -> {:?}", now_ts(), request, response);
                    }
                    reply.set_result(response.status.to_u16() as i32);
                    reply.set_continued(true);
                    if request.headers {
                        for hv in response.headers.iter() {
                            reply.set_headers(hv.to_string().into_bytes());
                            try!(Trawler::reply(sock, &request.client, &reply));
                        }
                        reply.clear_headers();
                    }
                    loop {
                        let mut buffer = [0u8; 0x4000];
                        let len = match response.read(&mut buffer[..]) {
                            Ok(i) => i,
                            Err(_) => {
                                return Ok(try!(Trawler::fail(sock, &request.client, &mut reply)));
                            }
                        };
                        if len == 0 {
                            break;
                        }
                        reply.set_response(buffer[..len].to_vec());
                        try!(Trawler::reply(sock, &request.client, &reply));
                    }
                    reply.clear_response();
                    reply.set_continued(false);
                    return Ok(try!(Trawler::reply(sock, &request.client, &reply)));
                },
            }
        }
        try!(Trawler::fail(sock, &request.client, &mut reply));
        return res;
      })
    }
    fn fail(sock: &mut zmq::Socket, client: &[u8], reply: &mut trawler::Reply) -> Result<(), TrawlerError> {
        reply.set_result(0i32);
        reply.clear_response();
        reply.set_continued(false);
        Trawler::reply(sock, client, reply)
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
