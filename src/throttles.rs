use time::*;

use trawler;
use trawler::Request_Method as Method;
use hyper::header::{Header,Cookie};

use std::collections::{LinkedList,VecDeque};
use regex::Regex;

use std::fmt;

use std::cmp::{max};

use std::time::Duration as sDuration;
use std::thread::sleep as tsleep;

fn sleep(to_sleep: Duration) {
    tsleep(sDuration::new(
        to_sleep.num_seconds() as u64,
        (to_sleep.num_nanoseconds().expect("sleep duration too large") - to_sleep.num_seconds() * 1_000_000_000) as u32
    ))
}

pub struct TRequest {
    pub client: Vec<u8>,
    pub id: i32,
    pub method: Method,
    pub path: String,
    pub query: String,
    pub session: Option<Cookie>,
    pub headers: bool,
    pub unthrottled: bool,
}

pub struct Throttle {
    pattern: Regex,
    window: Duration,
    limit: usize,
    spacing: Duration,
    requests: LinkedList<TRequest>,
    last_start: PreciseTime,
    last_ends: VecDeque<PreciseTime>,
    subthrottles: Vec<Throttle>,
    unthrottled: LinkedList<TRequest>,
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
                .field("subthrottles", &self.subthrottles)
            .finish()
    }
}

impl Throttle {
    pub fn nssite() -> Throttle {
        Throttle::new(Regex::new(r"^.*$").unwrap(),
            Duration::seconds(30),
            55,
            vec![Throttle::nsapi(),Throttle::nsnonapi()])
    }
    fn nsnonapi() -> Throttle {
        Throttle::new(Regex::new(r"^.*$").unwrap(),
            Duration::minutes(1),
            10,
            vec![])
    }
    fn nsapi() -> Throttle {
        Throttle::new(Regex::new(r"^/?cgi-bin/api\.cgi").unwrap(),
            Duration::seconds(31),
            50,
            vec![])
    }
    fn new(pattern: Regex, window: Duration, limit: usize, subthrottles: Vec<Throttle> ) -> Throttle {
        let min_margin = Duration::seconds(1);
        Throttle {
            pattern: pattern,
            window: window,
            limit: limit,
            spacing: (window + min_margin)/(limit as i32),
            requests: LinkedList::new(),
            last_start: PreciseTime::now(),
            last_ends: VecDeque::with_capacity(limit),
            subthrottles: subthrottles,
            unthrottled: LinkedList::new(),
        }
    }
    fn record_end( last_ends: &mut VecDeque<PreciseTime>, limit: usize ) {
        let end = PreciseTime::now();
        if last_ends.len() == limit {
            last_ends.pop_back();
        }
        last_ends.push_front( end );
    }
    pub fn span_request<F,T>(&mut self, mut inner: F ) -> T 
        where F: FnMut(TRequest) -> T {
        match self.unthrottled.pop_front() {
            Some(request) => return inner(request),
            None => {
                if self.requests.is_empty() {
                    if self.is_empty() {
                        panic!("span_request called when no requests available to span!");
                    }
                    loop {
                        let subthrottle_tm = self.subthrottle_timeout();
                        if subthrottle_tm > Duration::zero() {
                            sleep(subthrottle_tm);
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        self.last_start = PreciseTime::now();
        for subthrottle in self.subthrottles.iter_mut() {
            if subthrottle.timeout() <= Duration::zero() {
                let res = subthrottle.span_request(inner);
                Throttle::record_end( &mut self.last_ends, self.limit );
                return res;
            }
        }
        let res = if let Some(request) = self.requests.pop_front() {
            inner(request)
        } else {
            panic!("span_request called when no requests available to span!");
        };
        Throttle::record_end( &mut self.last_ends, self.limit );
        res
    }
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty() && 
            self.subthrottles.iter().all(|s| s.is_empty() )
    }
    fn local_timeout(&self) -> Duration {
        let now = PreciseTime::now();
        let spaced = self.spacing - (self.last_start.to(now));
        if let Some(first_end) = self.last_ends.get(self.limit - 1) {
            max(spaced, self.window - (first_end.to(now)))
        } else {
            spaced
        }
    }
    fn subthrottle_timeout(&self) -> Duration {
        self.subthrottles.iter().map(|s| s.timeout()).min().unwrap()
    }
    pub fn timeout(&self) -> Duration {
        let local_tm = self.local_timeout();
        if self.is_empty() {
            Duration::max_value()
        } else if self.subthrottles.len() > 0 {
            let subthrottle_tm = self.subthrottle_timeout();
            if self.requests.len() > 0 {
                max(subthrottle_tm, local_tm)
            } else {
                subthrottle_tm
            }
        } else {
            local_tm
        }
    }
    pub fn queue_request(&mut self, request: TRequest ) -> bool {
        if self.pattern.is_match(&*request.path) {
            for subthrottle in self.subthrottles.iter_mut() {
                if subthrottle.pattern.is_match(&*request.path) {
                    return subthrottle.queue_request(request);
                }
            }
            self.requests.push_back(request);
            return true;
        }
        return false;
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
            session: if preq.has_session() && preq.get_session().len() > 0 {
                Some(TRequest::cookie(preq.get_session()))
            } else {
                None
            },
            headers: preq.has_headers() && preq.get_headers(),
            unthrottled: preq.has_user_click(),
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
