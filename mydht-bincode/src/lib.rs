#[macro_use] extern crate log;
#[macro_use] extern crate mydht_base;
extern crate bincode;
extern crate serde;
#[cfg(test)]
extern crate mydht_basetest;

use serde::{Serializer,Serialize,Deserialize,Deserializer};
use serde::de::{DeserializeOwned};
//use rustc_serialize::{Serialize,Decodable};
use mydht_base::msgenc::MsgEnc;
use mydht_base::keyval::{KeyVal,Attachment};
use mydht_base::peer::{Peer};
use mydht_base::msgenc::ProtoMessage;
//use std::collections::BTreeMap;
use std::io::Write;
use std::io::Read;
use mydht_base::msgenc::write_attachment;
use mydht_base::msgenc::read_attachment;
use mydht_base::mydhtresult::Result as MDHTResult;
use mydht_base::mydhtresult::{Error,ErrorKind};
use mydht_base::msgenc::send_variant::ProtoMessage as ProtoMessageSend;
use bincode::Infinite;
use std::error::Error as StdError;

//use std::marker::Reflect;
use bincode::Error as BinError;

pub struct BinErr(BinError);
impl From<BinErr> for Error {
  #[inline]
  fn from(e : BinErr) -> Error {
    Error(e.0.description().to_string(), ErrorKind::SerializingError, Some(Box::new(e.0)))
  }
}


// full bencode impl
#[derive(Debug,Clone)]
pub struct Bincode;

impl<P : Peer, M : Serialize + DeserializeOwned> MsgEnc<P,M> for Bincode {

/*  fn encode<P : Peer, V : KeyVal> (&self, mesg : &ProtoMessage<P,V>) -> Option<Vec<u8>>{
    debug!("encode msg {:?}", mesg);
    let r = bincode::encode(mesg, bincode::SizeLimit::Infinite);
    r.ok()
  }*/

/*  fn decode<P : Peer, V : KeyVal> (&self, buff : &[u8]) -> Option<ProtoMessage<P,V>>{
    debug!("decode msg {:?}", buff);
    bincode::decode(buff).ok()
  }*/

  fn encode_into<'a,W : Write> (&self, w : &mut W, mesg : &ProtoMessageSend<'a,P>) -> MDHTResult<()>
where <P as Peer>::Address : 'a,
      <P as KeyVal>::Key : 'a {
  
     tryfor!(BinErr,bincode::serialize_into(w, mesg, Infinite));
     Ok(())
  }
  fn encode_msg_into<W : Write> (&self, w : &mut W, mesg : &M) -> MDHTResult<()> {
     tryfor!(BinErr,bincode::serialize_into(w, mesg, Infinite));
     Ok(())
  }


  fn attach_into<W : Write> (&self, w : &mut W, a : &Attachment) -> MDHTResult<()> {
//    try!(w.write_all(&[if a.is_some(){1}else{0}]));
    write_attachment(w,a)
  }

  fn decode_from<R : Read>(&self, r : &mut R) -> MDHTResult<ProtoMessage<P>> {
    Ok(tryfor!(BinErr,bincode::deserialize_from(r, Infinite)))
  }
  fn decode_msg_from<R : Read>(&self, r : &mut R) -> MDHTResult<M> {
    Ok(tryfor!(BinErr,bincode::deserialize_from(r, Infinite)))
  }


  fn attach_from<R : Read>(&self, r : &mut R, mlen : usize) -> MDHTResult<Attachment> {
/*    let mut buf = [0];
    try!(r.read(&mut buf));
    match buf[0] {
      i if i == 0 => Ok(None),
      i if i == 1 => {
          debug!("Reding an attached file");
          read_attachment(r).map(|a|Some(a))
      },
      _ => return Err(Error("Invalid attachment description".to_string(), ErrorKind::SerializingError, None)),
    }*/
    read_attachment(r,mlen)
  }

}

#[test]
fn test_binc_apeer () {
  mydht_basetest::msgenc::test_peer_enc(Bincode);
}
