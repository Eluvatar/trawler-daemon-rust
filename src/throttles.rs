use time::*;

use trawler;
use trawler::Request_Method as Method;
use hyper::header::{Header,Cookie};

use std::collections::{LinkedList,VecDeque};
use regex::Regex;

use std::fmt;

use std::cmp::{max};

pub struct TRequest {
    pub client: Vec<u8>,
    pub id: i32,
    pub method: Method,
    pub path: String,
    pub query: String,
    pub session: Option<Cookie>,
    pub headers: bool,
}

pub struct Throttle {
    pattern: Regex,
    window: Duration,
    limit: usize,
    min_margin: Duration,
    spacing: Duration,
    requests: LinkedList<TRequest>,
    last_start: PreciseTime,
    last_ends: VecDeque<PreciseTime>,
// TODO    subthrottles: Vec<Throttle>,
}

fn time_since(ts: &PreciseTime) -> String {
    format!("{} ago", (*ts).to(PreciseTime::now()))
}

impl fmt::Debug for Throttle {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("Throttle")
            .field("pattern", &self.pattern)
            .field("window", &self.window)
            .field("limit", &self.limit)
            .field("spacing", &self.spacing)
            .field("requests", &self.requests)
            .field("last_start", &time_since(&self.last_start))
            .field("last_ends", &self.last_ends.iter().map(time_since)
                .collect::<Vec<String>>())
// TODO             .field("subthrottles", &self.subthrottles)
            .finish()
    }
}

impl Throttle {
    pub fn nsapi() -> Throttle {
        Throttle::new(Regex::new(r"^.*$").unwrap(),
            Duration::seconds(30),
            50)
    }
    fn new(pattern: Regex, window: Duration, limit: usize ) -> Throttle {
        let min_margin = Duration::seconds(1);
        Throttle {
            pattern: pattern,
            window: window,
            limit: limit,
            min_margin: min_margin,
            spacing: (window + min_margin)/(limit as i32),
            requests: LinkedList::new(),
            last_start: PreciseTime::now(),
            last_ends: VecDeque::with_capacity(limit),
        }
    }
    pub fn span_request<F,T>(&mut self, mut inner: F ) -> T 
        where F: FnMut(TRequest) -> T {
        self.last_start = PreciseTime::now();
        let res = inner(self.requests.pop_front().unwrap());
        let end = PreciseTime::now();
        if self.last_ends.len() == self.limit {
            self.last_ends.pop_back();
        }
        self.last_ends.push_front( end );
        res
    }
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
    pub fn timeout(&self) -> Duration {
        let now = PreciseTime::now();
        let spaced = self.spacing - (self.last_start.to(now));
        if let Some(first_end) = self.last_ends.get(self.limit - 1) {
            max(spaced, self.window - (first_end.to(now)))
        } else {
            spaced
        }
    }
    pub fn queue_request(&mut self, request: TRequest ) {
        // TODO subthrottles
        self.requests.push_back(request);
    }
}
impl TRequest {
    pub fn new(client: &[u8], preq: &trawler::Request) -> TRequest {
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
