use std::io::Result as IoResult;
use std::sync::mpsc::{Receiver,Sender};
use rustc_serialize::{Encoder,Encodable,Decoder,Decodable};
use peer::{PeerMgmtRules, PeerPriority};
use query::{self,QueryConf,QueryRules,QueryPriority,QueryMode,QueryModeMsg,LastSent};
use kvstore::{StoragePriority, KeyVal, KVStore};
use query::cache::QueryCache;
use std::str::from_utf8;
use rustc_serialize::json;
use self::mesgs::{PeerMgmtMessage,KVStoreMgmtMessage,QueryMgmtMessage};
use std::str::FromStr;
use std::sync::{Arc,Semaphore,Mutex,Condvar};
use std::sync::mpsc::channel;
use std::thread::{JoinGuard};
use std::thread;
use std::num::{ToPrimitive,Int};
use route::Route;
use peer::Peer;
use transport::{TransportStream,Transport};
use time;
use std::time::Duration as OldDuration;
use utils::{self,OneResult};
use msgenc::MsgEnc;

pub mod mesgs;
mod server;
mod client;
mod peermanager;
mod kvmanager;
mod querymanager;

//pub type ClientChanel<P : Peer, V : KeyVal> = Sender<mesgs::ClientMessage<P,V>>;
pub type ClientChanel<P, V> = Sender<mesgs::ClientMessage<P,V>>;

/// Running context contain all information needed, mainly configuration and calculation rules.
/// There is a need for its content to be sync (we share context in an Arc (preventing us from
/// cloning its content and therefore requiring sync to be use in multiple thread).
/// TODO remove Option<V> (retest with new compiler : without it says V is unused yet used in
/// peermgmtrules)
//pub type RunningContext<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> = Arc<(Arc<P>, R, Q, E, T, Option<V>)>;
pub type RunningContext<P, V, R, Q, E, T> = Arc<(Arc<P>, R, Q, E, T, Option<V>)>;

/// Channel used by several process, they are cloned/moved when send to new thread (sender are not
/// sync)
//pub type RunningProcesses<P : Peer, V : KeyVal> = (Sender<mesgs::PeerMgmtMessage<P,V>>, Sender<QueryMgmtMessage<P,V>>, Sender<mesgs::KVStoreMgmtMessage<P,V>>);
pub type RunningProcesses<P, V> = (Sender<mesgs::PeerMgmtMessage<P,V>>, Sender<QueryMgmtMessage<P,V>>, Sender<mesgs::KVStoreMgmtMessage<P,V>>);


// TODO replace f to Arc<Condvar>
pub struct DHT<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> {rp : RunningProcesses<P,V>, rc : RunningContext<P,V,R,Q,E,T>, f : Arc<Semaphore>}


/// Find a value by key. Specifying our queryconf, and priorities.
pub fn find_local_val<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> (rp : &RunningProcesses<P,V>, rc : &RunningContext<P,V,R,Q,E,T>, nid : V::Key ) -> Option<V> {
  debug!("Finding KeyVal locally {:?}", nid);
  let sync = Arc::new((Mutex::new(None),Condvar::new()));
  // local query replyto set to None
  rp.2.send(KVStoreMgmtMessage::KVFindLocally(nid, Some(sync.clone())));
  // block until result
  utils::clone_wait_one_result(sync).unwrap_or(None)
}

/// Store a value. Specifying our queryconf, and priorities. Note that priority rules are very
/// important to know if we do propagate value or store local only or cache local only.
pub fn store_val <P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> (rp : &RunningProcesses<P,V>, rc : &RunningContext<P,V,R,Q,E,T>, val : V, (qmode, qchunk, lsconf) : QueryConf, prio : QueryPriority, sprio : StoragePriority) -> bool {
  let msgqmode = init_qmode(rp, rc, &qmode);
  //let lastsent = lsconf.map(|n| LastSent(n,Vec::new()));
  let lastsent = lsconf.map(|(n,ishop)| if ishop 
    {LastSent::LastSentHop(n,vec![rc.0.get_key()].into_iter().collect())}
    else
    {LastSent::LastSentPeer(n,vec![rc.0.get_key()].into_iter().collect())}
  );
  let maxhop = rc.2.nbhop(prio);
  let nbquer = rc.2.nbquery(prio);
  let queryconf = (msgqmode.clone(), qchunk, lastsent, sprio,maxhop,nbquer,prio,1);
  let sync = Arc::new((Mutex::new(false),Condvar::new()));
  // for propagate 
  rp.2.send(KVStoreMgmtMessage::KVAddPropagate(val,Some(sync.clone()),queryconf));
  // TODO wait for propagate result...??? plus new message cause storekv is
  // wait for conflict management issue reply TODO instead of simple bool
  // for local
  match utils::clone_wait_one_result(sync){
    None => {error!("Condvar issue for storing value!!"); false},// not logic 
    Some (r) => r,
  }
}


/// Find a value by key. Specifying our queryconf, and priorities.
pub fn find_val<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> (rp : &RunningProcesses<P,V>, rc : &RunningContext<P,V,R,Q,E,T>, nid : V::Key, (qmode, qchunk, lsconf) : QueryConf, prio : QueryPriority, sprio : StoragePriority, nb_res : usize ) -> Vec<Option<V>> {
  debug!("Finding KeyVal {:?}", nid);
  // TODO factorize code with find peer and/or specialize rules( some for peer some for kv) ??
  let maxhop = rc.2.nbhop(prio);
  let nbquer = rc.2.nbquery(prio);
  let semsize = match qmode {
    QueryMode::Asynch => Int::pow(nbquer.to_uint().unwrap(), maxhop.to_u32().unwrap()),
    // general case we wait reply in each client query
    _ => nbquer.to_uint().unwrap(),
  };
  let msgqmode = init_qmode(rp, rc, &qmode);
  let lifetime = rc.2.lifetime(prio);
  let managed =  msgqmode.clone().get_qid(); // TODO redesign get_qid to avoid clone
  let lastsent = lsconf.map(|(n,ishop)| if ishop 
    {LastSent::LastSentHop(n,vec![rc.0.get_key()].into_iter().collect())}
    else
    {LastSent::LastSentPeer(n,vec![rc.0.get_key()].into_iter().collect())}
  );
  let store = rc.2.do_store(true, prio, sprio, Some(0)); // first hop
  let queryconf = (msgqmode, qchunk, lastsent, sprio,maxhop,nbquer,prio,nb_res);
  // local query replyto set to None
  let query = query::init_query(semsize, nb_res, lifetime, & rp.1, None, managed,Some(store));
  rp.2.send(KVStoreMgmtMessage::KVFind(nid,Some(query.clone()), queryconf));
  // block until result
  query.wait_query_result().right().unwrap()
}

#[inline]
fn init_qmode<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, T : Transport> (rp : &RunningProcesses<P,V>, rc : &RunningContext<P,V,R,Q,E,T>, qm : &QueryMode) -> QueryModeMsg <P>{
  match qm {
    &QueryMode::Proxy => QueryModeMsg::Proxy, 
    &QueryMode::Asynch => QueryModeMsg::Asynch((rc.0).clone(),rc.2.newid()),
    &QueryMode::AProxy => QueryModeMsg::AProxy((rc.0).clone(),rc.2.newid()),
    &QueryMode::AMix(i) => QueryModeMsg::AMix(i,rc.0.clone(),rc.2.newid()),
  }
}

impl<P : Peer, V : KeyVal, R : PeerMgmtRules<P, V>, Q : QueryRules, E : MsgEnc, TT : Transport> DHT<P, V, R, Q, E, TT> {
  pub fn block (&self) {
    debug!("Blocking");
    self.f.acquire();
  }
  pub fn shutdown (&self) {
    debug!("Sending Shutdown");
    self.rp.2.send(KVStoreMgmtMessage::Shutdown);
    self.rp.0.send(PeerMgmtMessage::ShutDown);
  }
  // reping offline closest peers  TODO refactor so that we refresh until target size not
  // returning nb of connection
  pub fn refresh_closest_peers(&self, targetNb : usize) -> usize {
    self.rp.0.send(PeerMgmtMessage::Refresh(targetNb));
    // TODO get an appropriate response
    0
  }

  #[inline]
  fn init_qmode(&self, qm : &QueryMode) -> QueryModeMsg <P>{
    init_qmode(&self.rp, &self.rc, qm)
  }

  pub fn find_peer (&self, nid : P::Key, (qmode, qchunk, lsconf) : QueryConf, prio : QueryPriority ) -> Option<Arc<P>>  {
    debug!("Finding peer {:?}", nid);
    let maxhop = self.rc.2.nbhop(prio);
    println!("!!!!!!!!!!!!!!!!!!! maxhop : {}, prio : {}", maxhop, prio);
    let nbquer = self.rc.2.nbquery(prio);
    let semsize = match qmode {
      QueryMode::Asynch => Int::pow(nbquer.to_uint().unwrap(), maxhop.to_u32().unwrap()),
      // general case we wait reply in each client query
      _ => nbquer.to_uint().unwrap(),
    };
    let msgqmode = self.init_qmode(&qmode);
    let lifetime = self.rc.2.lifetime(prio);
    let managed =  msgqmode.clone().get_qid(); // TODO redesign get_qid to avoid clone
    let lastsent = lsconf.map(|(n,ishop)| if ishop 
      {LastSent::LastSentHop(n,vec![self.rc.0.get_key()].into_iter().collect())}
    else
      {LastSent::LastSentPeer(n,vec![self.rc.0.get_key()].into_iter().collect())}
    );
    let nb_res = 1;
    let queryconf = (msgqmode.clone(), qchunk, lastsent,  StoragePriority::All ,maxhop,nbquer,prio,nb_res); // querystorage priority is hadcoded but not used to (peer are curently always stored) TODO switch to option??
    // local query replyto set to None
    let query = query::init_query(semsize, nb_res, lifetime, & self.rp.1, None, managed, None); // Dummy store policy
//pub fn init_query (semsize : int, lifetime : Duration, rp : RunningProcesses, replyto : Option<QueryConfMsg>, managed : Option<QueryID>) -> Query<Node> 
    self.rp.0.send(PeerMgmtMessage::PeerFind(nid,Some(query.clone()), queryconf));
    // block until result
    query.wait_query_result().left().unwrap()

  }


  // at the time only query without persistence and intermediatory persistence strategy (related
  // to route strategy)
  // Notice that most of the time V must be defined as an Arc of something with serialize
  // implementation on its content (there is quite a lot of clone involved).
  /// Find a value by key. Specifying our queryconf, and priorities.
  #[inline]
  pub fn find_val (&self, nid : V::Key, qc : QueryConf, prio : QueryPriority, sprio : StoragePriority, nb_res : usize ) -> Vec<Option<V>> {
    find_val(&self.rp, &self.rc, nid, qc, prio, sprio, nb_res)
  }

  // at the time only local without propagation strategy return true if ok (todo variant with or
  // without propagation result)
  /// Store a value. Specifying our queryconf, and priorities. Note that priority rules are very
  /// important to know if we do propagate value or store local only or cache local only.
  #[inline]
  pub fn store_val (&self, val : V, qc : QueryConf, prio : QueryPriority, sprio : StoragePriority) -> bool {
    store_val(&self.rp, &self.rc, val, qc, prio, sprio)
  }

/// Main function to start a DHT.
pub fn boot_server
 <T : Route<P,V>, 
  QC : QueryCache<P,V>, 
  S : KVStore<V>,
  F : FnOnce() -> Option<S> + Send + 'static,
 >
 (rc : RunningContext<P,V,R,Q,E,TT>, 
  mut route : T, 
  mut querycache : QC, 
  mut kvst : F,
  cachedNodes : Vec<Arc<P>>, 
  bootNodes : Vec<Arc<P>>,
  ) 
 -> DHT<P,V,R,Q,E,TT> {

let (tquery,rquery) = channel();
let (tkvstore,rkvstore) = channel();
let (tpeer,rpeer) = channel();
let cleandelay = rc.2.asynch_clean();
let cleantquery = tquery.clone();
let resulttquery = tquery.clone();
let cleantpeer = tpeer.clone();
let cleantkstor = tkvstore.clone();

// Query manager is allways start TODO a parameter for not starting it (if running dht in full
// proxy mode for instance)
thread::spawn (move ||{
  querymanager::start(&rquery, &cleantquery, &cleantpeer, &cleantkstor, querycache, cleandelay);
});
let sem = Arc::new(Semaphore::new(-1)); // wait end of two process from shutdown

let rp = (tpeer.clone(), tquery.clone(),tkvstore.clone());
let tpeer3 = tpeer.clone();
// starting peer manager process
let rcsp = rc.clone();
let rpsp = rp.clone();
let semsp = sem.clone();
thread::spawn (move ||{
  peermanager::start::<_,_,_,_,_,_,TT> (rcsp, route, &rpeer,rpsp, semsp)
});

// starting kvstore process
let rcst = rc.clone();
let rpst = rp.clone();
let semsp2 = sem.clone();
thread::spawn (move ||{
  kvmanager::start (rcst, kvst, &rkvstore,rpst, semsp2);
});

// starting socket listener process
let tpeer2 = tpeer3.clone();
let tpeer4 = tpeer3.clone();
let rcsp2 = rc.clone();
let rpsp2 = rp.clone();
thread::spawn (move ||{
  server::servloop::<_,_,_,_,_,TT>(rcsp2, rpsp2)
});

// Typically those cached node are more likely to be initialized with the routing backend (here it
// is slower as we need to clone nodes)
info!("loading additional cached node {:?}", cachedNodes);
for p in cachedNodes.iter() {
  tpeer3.send(PeerMgmtMessage::PeerAdd(p.clone(),PeerPriority::Offline));
}

info!("bootstrapping with {:?}", bootNodes);
for p in bootNodes.iter() {
  tpeer3.send(PeerMgmtMessage::PeerPing(p.clone(),None)); // TODO avoid cloning node... eg try maping to collection of arc
}

DHT{
  rp : (tpeer4,resulttquery,tkvstore),
  rc : rc,
  f : sem
}

}

}
