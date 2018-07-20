use futures::{prelude::*, sync::oneshot};
use tokio::{self, prelude::*, spawn, net::{TcpStream, TcpListener}};
use tokio_io::io::{WriteHalf, write_all};
use tokio_timer::Interval;
use std::{
    io::BufReader, net::SocketAddr, sync::{Arc, RwLock, Mutex}, result,
    time::{Instant, Duration},
    collections::{HashMap, HashSet, BTreeSet}
};
use path::Path;
use utils::{BatchItem, batched};
use serde::Serialize;
use serde_json;
use resolver_store::{Action, Store};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientHello {
    ReadOnly,
    ReadWrite { ttl: i64, write_addr: SocketAddr }
}
 

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerHello { pub ttl_expired: bool }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ToResolver {
    Resolve(Path),
    List(Path),
    Publish(Vec<Path>),
    Unpublish(Vec<Path>),
    Clear
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum FromResolver {
    Resolved(Vec<SocketAddr>),
    List(Vec<Path>),
    Published,
    Unpublished,
    Error(String)
}

#[async]
fn send<T: Serialize + 'static>(
    w: WriteHalf<TcpStream>, m: T
) -> result::Result<WriteHalf<TcpStream>, ()> {
    let m = serde_json::to_vec(&m).map_err(|_| ())?;
    let w = await!(write_all(w, m)).map_err(|_| ())?.0;
    Ok(await!(write_all(w, "\n")).map_err(|_| ())?.0)
}

struct ClientInfoInner {
    addr: SocketAddr
    ttl: Duration,
    last: Instant,
    published: BTreeSet<Path>,
    stop: Option<oneshot::Sender<()>>,
}

struct ClientInfo(Arc<Mutex<ClientInfoInner>>);

impl ClientInfo {
    fn new(addr: SocketAddr, ttl: u64, stop: oneshot::Sender<()>) -> Self {
        let inner = ClientInfoInner {
            addr,
            ttl: Duration::from_secs(ttl),
            last: Instant::now(),
            published: BTreeSet::new(),
            stop: Some(stop),
        };
        ClientInfo(Arc::new(Mutex::new(inner)))
    }
}

impl Clone for ClientInfo {
    fn clone(&self) -> Self { ClientInfo(Arc::clone(&self.0)) }
}

struct Stops {
    stops: HashMap<usize, oneshot::Sender<()>>,
    stop_id: usize,
}

impl Stops {
    fn new() -> Self {
        Stops {
            stops: HashMap::new(),
            stop_id: 0
        }
    }

    fn make(&mut self) -> (oneshot::Receiver<()>, usize) {
        let (tx, rx) = oneshot::channel();
        let id = self.stop_id;
        self.stops.insert(id, tx);
        self.stop_id += 1;
        (rx, id)
    }

    fn remove(&mut self, id: &usize) { self.stops.remove(id); }

    fn stop(&mut self) {
        for (_, s) in self.stops.drain() { let _ = s.send(()); }
    }
}

#[derive(Clone)]
struct Context {
    published: Store,
    clients: Arc<RwLock<HashMap<SocketAddr, ClientInfo>>>,
    stops: Arc<Mutex<Stops>>
}

impl Context {
    fn timeout_client(&self, client: &mut ClientInfoInner) {
        let mut stop = None;
        ::std::mem::swap(&mut client.stop, &mut stop);
        if let Some(stop) = stop { let _ = stop.send(()); }
        self.published.change(client.published.iter().map(|p| {
            (p, (Action::Unpublish, client.addr))
        }))
    }

    fn publish(
        &self, mut paths: Vec<Path>, client: &mut ClientInfoInner
    ) -> FromResolver {
        paths.sort_unstable();
        if !paths.iter().all(|p| p.is_absolute()) {
            return FromResolver::Error("publish relative path".into())
        }
        for p in &paths { client.insert(p.clone()); }
        self.published.change(paths.into_iter().map(|p| {
            (p, (Action::Publish, client.addr))
        }));
        FromResolver::Published
    }

    fn unpublish(
        &self, mut paths: Vec<Path>, client: &mut ClientInfoInner
    ) -> FromResolver {
        paths.sort_unstable();
        for p in &paths { client.published.remove(p); }
        self.published.unpublish(paths.into_iter().map(|p| {
            (p, (Action::Unpublish, client.addr))
        }));
        FromResolver::Unpublished
    }
}

#[async]
fn handle_client(
    ctx: Context, s: TcpStream, server_stop: oneshot::Receiver<()>
) -> result::Result<(), ()> {
    enum M { Stop, Line(String) }
    let addr = s.peer_addr().map_err(|_| ())?;
    s.set_nodelay(true).map_err(|_| ())?;
    let (rx, mut tx) = s.split();
    let msgs =
        tokio::io::lines(BufReader::new(rx)).map_err(|_| ()).map(|l| M::Line(l))
        .select(server_stop.into_stream().map_err(|_| ()).map(|()| M::Stop));
    let (hello, msgs) =
        match await!(msgs.into_future()) {
            Err(..) => return Err(()),
            Ok((None, _)) => return Err(()),
            Ok((Some(M::Stop), _)) => return Ok(()),
            Ok((Some(M::Line(l)), msgs)) =>
                (serde_json::from_str::<ClientHello>(&l).map_err(|_| ())?, msgs)
        };
    let (client, client_stop, mut client_added) = {
        let (tx_stop, rx_stop) = oneshot::channel();
        let (client, added, ttl_expired) =
            match hello {
                ClientHello::ReadOnly =>
                    (ClientInfo::new(addr, 120, tx_stop), false, false),
                ClientHello::ReadWrite {ttl, write_addr} => {
                    if ttl <= 0 || ttl > 3600 { return Err(()) }
                    match ctx.clients.read().unwrap().get(&write_addr) {
                        None => {
                            let c = ClientInfo::new(write_addr, ttl as u64, tx_stop);
                            (c, false, true)
                        },
                        Some(client) => {
                            let mut cl = client.0.lock().unwrap();
                            cl.last = Instant::now();
                            cl.stop = Some(tx_stop);
                            (client.clone(), true, false)
                        }
                    }
                }
            };
        tx = await!(send::<ServerHello>(tx, ServerHello { ttl_expired }))?;
        (client, rx_stop, added)
    };
    let msgs = msgs.select(client_stop.into_stream().map_err(|_| ()).map(|_| M::Stop));
    let msgs = batched(msgs, 10000);
    let mut batch : Vec<ToResolver> = Vec::new();
    let mut response : Vec<FromResolver> = Vec::new();
    let mut batch_needs_write_lock = false;
    #[async]
    for msg in msgs {
        match msg {
            BatchItem::InBatch(m) =>
                match m {
                    M::Stop => break,
                    M::Line(l) =>
                        match serde_json::from_str::<ToResolver>(&l).map_err(|_| ())? {
                            m@ ToResolver::Resolve(..) | m@ ToResolver::List(..) =>
                                batch.push(m),
                            m@ ToResolver::Publish(..) | m@ ToResolver::Unpublish(..) => {
                                batch_needs_write_lock = true;
                                batch.push(m)
                            }
                        }
                },
            BatchItem::EndBatch => {         
                if batch_needs_write_lock {
                    let mut t = ctx.0.write().unwrap();
                    let mut ci = client.0.lock().unwrap();
                    ci.last = Instant::now();
                    if !client_added {
                        client_added = true;
                        match hello {
                            ClientHello::ReadOnly => return Err(()),
                            ClientHello::ReadWrite {write_addr, ..} =>
                                t.clients.insert(write_addr, client.clone());
                        }
                    }
                    for m in batch.drain(0..) {
                        match m {
                            ToResolver::Resolve(ref path) =>
                                response.push(t.resolve(path)),
                            ToResolver::List(ref path) => response.push(t.list(path)),
                            ToResolver::Publish(path, addr) =>
                                response.push(t.publish(path, addr, &mut ci)),
                            ToResolver::Unpublish(path, addr) =>
                                response.push(t.unpublish(path, addr,  &mut ci)),
                        }
                    }
                } else {
                    let t = ctx.0.read().unwrap();
                    {
                        let mut ci = client.0.lock().unwrap();
                        ci.last = Instant::now();
                    }
                    for m in batch.drain(0..) {
                        match m {
                            ToResolver::Resolve(ref path) =>
                                response.push(t.resolve(path)),
                            ToResolver::List(ref path) => response.push(t.list(path)),
                            ToResolver::Publish(..) | ToResolver::Unpublish(..) =>
                                unreachable!("write lock required")
                        }
                    }
                }
                while let Some(m) = response.pop() {
                    tx = await!(send(tx, m)).map_err(|_| ())?;
                }
                batch_needs_write_lock = false;
            }
        }
    }
    Ok(())
}

#[async]
fn start_client(
    ctx: Context, s: TcpStream,
    client: usize,
    server_stop: oneshot::Receiver<()>,
) -> result::Result<(), ()> {
    let _ = await!(handle_client(ctx.clone(), s, server_stop));
    ctx.0.write().unwrap().stops.remove(&client);
    Ok(())
}

#[async]
fn client_scavenger(
    ctx: Context, stop: oneshot::Receiver<()>
) -> result::Result<(), ()> {
    enum M { Tick(Instant), Stop }
    let msgs =
        Interval::new(Instant::now(), Duration::from_secs(10))
        .map_err(|_| ())
        .map(|i| M::Tick(i))
        .select(stop.into_stream().map_err(|_| ()).map(|_| M::Stop));
    let mut check: Vec<(Uuid, ClientInfo)> = Vec::new();
    let mut delete: Vec<Uuid> = Vec::new();
    #[async]
    for m in msgs {
        match m {
            M::Stop => break,
            M::Tick(now) => {
                let mut t = ctx.0.write().unwrap();
                for (id, client) in t.clients.iter() { check.push((*id, client.clone())) }
                for (id, client) in check.drain(0..) {
                    let mut cl = client.0.lock().unwrap();
                    if now - cl.last > cl.ttl {
                        t.timeout_client(&mut cl);
                        delete.push(id);
                    }
                }
                for id in delete.drain(0..) { t.clients.remove(&id); }
            }
        }
    }
    Ok(())
}

#[async]
fn accept_loop(
    addr: SocketAddr,
    stop: oneshot::Receiver<()>,
    ready: oneshot::Sender<()>,
) -> result::Result<(), ()> {
    let t : Context =
        Context(Arc::new(RwLock::new(ContextInner {
            published: Store::new(),
            clients: HashMap::new(),
            stops: Stops::new(),
        })));
    enum M { Stop, Client(TcpStream) }
    let msgs =
        TcpListener::bind(&addr).map_err(|_| ())?
        .incoming().map_err(|_| ()).map(|c| M::Client(c))
        .select(stop.into_stream().map_err(|_| ()).map(|()| M::Stop));
    let _ = ready.send(());
    spawn(client_scavenger(t.clone(), t.0.write().unwrap().stops.make().0));
    #[async]
    for msg in msgs {
        match msg {
            M::Stop => break,
            M::Client(client) => {
                let (stop, cid) = t.0.write().unwrap().stops.make();
                spawn(start_client(t.clone(), client, cid, stop));
            },
        }
    }
    let mut ctx = t.0.write().unwrap();
    ctx.stops.stop();
    Ok(())
}

pub struct Server(Option<oneshot::Sender<()>>);

impl Drop for Server {
    fn drop(&mut self) {
        let mut stop = None;
        ::std::mem::swap(&mut stop, &mut self.0);
        if let Some(stop) = stop { let _ = stop.send(()); }
    }
}

use error::*;

impl Server {
    #[async]
    pub fn new(addr: SocketAddr) -> Result<Server> {
        let (send_stop, recv_stop) = oneshot::channel();
        let (send_ready, recv_ready) = oneshot::channel();
        spawn(accept_loop(addr, recv_stop, send_ready));
        await!(recv_ready).map_err(|_| Error::from("ipc error"))?;
        Ok(Server(Some(send_stop)))
    }
}
