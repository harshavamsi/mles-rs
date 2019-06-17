/* This Source Code Form is subject to the terms of the Mozilla Public
*  License, v. 2.0. If a copy of the MPL was not distributed with this
*  file, You can obtain one at http://mozilla.org/MPL/2.0/.
*
*  Copyright (C) 2017-2019  Mles developers
* */
use std::io::{Error,ErrorKind};
use std::net::SocketAddr;
use std::time::Duration;
use std::str;
use std::borrow::Cow;

use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;
use std::mem;

use futures::stream::Stream;
use futures::sync::mpsc::unbounded;
use futures::{Future, Sink};
use tokio::net::TcpListener;
use tokio::runtime::current_thread::{Runtime, TaskExecutor};
use tungstenite::protocol::Message;
use tungstenite::handshake::server::Request;

use tokio_tungstenite::accept_hdr_async;
use crate::KEEPALIVE;
use tokio::net::TcpStream;
use tokio_codec::Decoder;
use crate::Bytes;
use mles_utils::*;

const WSPORT: &str = ":8076";
const SECPROT: &str = "Sec-WebSocket-Protocol";
const SECVALUE: &str = "mles-websocket";

pub fn process_ws_proxy(raddr: SocketAddr, keyval: String, keyaddr: String) {
    let addr = "0.0.0.0".to_string() + WSPORT;
    let addr = addr.parse().unwrap();
    let mut runtime = Runtime::new().unwrap();
    let socket = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(err) => {
            println!("Proxy error: {}", err);
            return;
        },
    };
    println!("Listening WebSockets on: {}", addr);
    let mut cnt = 0;

    let srv = socket.incoming().for_each(move |stream| {
        let _val = stream.set_nodelay(true)
                         .map_err(|_| panic!("Cannot set to no delay"));
        let _val = stream.set_keepalive(Some(Duration::new(KEEPALIVE, 0)))
                         .map_err(|_| panic!("Cannot set keepalive"));
        let addr = stream.local_addr().unwrap();
        cnt += 1;

        let (ws_tx, ws_rx) = unbounded();

        let keyval_inner = keyval.clone();
        let keyaddr_inner = keyaddr.clone();

        let callback = |req: &Request| {
            for &(ref header, ref value) in req.headers.iter() {
                if header == SECPROT {
                    let mut secstr = String::new();
                    for c in value.iter() {
                        let c = *c as char;
                        secstr.push(c);
                    }
                    //in case several protocol, split to vector of strings
                    let secvec: Vec<&str> = secstr.split(',').collect();
                    for secstr in secvec {
                        if secstr == SECVALUE {
                            let extra_headers = vec![
                                (SECPROT.to_string(), SECVALUE.to_string()),
                            ];
                            return Ok(Some(extra_headers));
                        }
                    }
                }
            }
            Err(tungstenite::Error::Protocol(Cow::from(SECPROT)))
        };
        //let mles_db_hash: HashMap<String, MlesDb> = HashMap::new();
        //let mles_db = Rc::new(RefCell::new(mles_db_hash));

        let accept = accept_hdr_async(stream, callback).map_err(|err|{
            println!("Accept error: {}", err);
            Error::new(ErrorKind::Other, err)
        });
        let accept = accept.and_then(move |ws_stream| {
            println!("New WebSocket connection {}: {}", cnt, addr);


            let (sink, stream) = ws_stream.split();
            let mut key: Option<u64> = None;
            let mut cid: Option<u32> = None;

            let ws_reader = stream.for_each(move |message: Message| {
                let mles_message = message.into_data();

                let keyval = keyval_inner.clone();
                let keyaddr = keyaddr_inner.clone();
                let ws_tx_inner = ws_tx.clone();
                let (mles_tx, mles_rx) = unbounded();

                if None == key {
                    println!("Key not found, connecting...");
                    let tcp = TcpStream::connect(&raddr);
                    println!("Connected.");

                    // Check if mles_message matches to local db
                    let client = tcp.and_then(move |stream| {
                        let mut keys = Vec::new();
                        let _val = stream.set_nodelay(true)
                            .map_err(|_| panic!("Cannot set to no delay"));
                        let _val = stream.set_keepalive(Some(Duration::new(KEEPALIVE, 0)))
                            .map_err(|_| panic!("Cannot set keepalive"));
                        let laddr = match stream.local_addr() {
                            Ok(laddr) => laddr,
                            Err(_) => {
                                let addr = "0.0.0.0:0";
                                addr.parse::<SocketAddr>().unwrap()
                            }
                        };
                        if  !keyval.is_empty() {
                            keys.push(keyval);
                        } else {
                            keys.push(MsgHdr::addr2str(&laddr));
                            if !keyaddr.is_empty() {
                                keys.push(keyaddr);
                            }
                        }

                        let (mut sink, stream) = Bytes.framed(stream).split();

                        let mles_rx = mles_rx.map_err(|_| panic!()); // errors not possible on rx XXX

                        let mles_rx = mles_rx.for_each(move |buf: Vec<_>| {
                            if None == key {
                                //create hash for verification
                                let decoded_message = Msg::decode(buf.as_slice());
                                keys.push(decoded_message.get_uid().to_string());
                                keys.push(decoded_message.get_channel().to_string());
                                key = Some(MsgHdr::do_hash(&keys));
                                cid = Some(MsgHdr::select_cid(key.unwrap()));
                            }

                            if buf.is_empty() {
                                println!("Empty buffer");
                                return Err(Error::new(ErrorKind::BrokenPipe, "broken pipe"));
                            }
                            let msghdr = MsgHdr::new(buf.len() as u32, cid.unwrap(), key.unwrap());
                            let mut msgv = msghdr.encode();
                            msgv.extend(buf);
                            let _ = sink.start_send(msgv).map_err(|err| {
                                Error::new(ErrorKind::Other, err)
                            });
                            let _ = sink.poll_complete().map_err(|err| {
                                Error::new(ErrorKind::Other, err)
                            });
                            Ok(())
                        });

                        //let send_wsrx = mles_rx.forward(sink);
                        let write_wstx = stream.for_each(move |buf| {
                            let ws_tx = ws_tx_inner.clone();
                            // send to websocket
                            let _ = ws_tx.send(buf.to_vec()).wait().map_err(|err| {
                                println!("Send error");
                                Error::new(ErrorKind::Other, err)
                            });
                            Ok(())
                        });

                        let send_wsrx = mles_rx.map(|_| {}).map_err(|_| {});
                        TaskExecutor::current().spawn_local(Box::new(send_wsrx.then(move |_| {
                            println!("Connection rx closed.");
                            Ok(())
                        }))).unwrap();
                        
                        let write_wstx = write_wstx.map(|_| ()).map_err(|_| {});
                        TaskExecutor::current().spawn_local(Box::new(write_wstx.then(move |_| {
                            println!("Connection tx closed.");
                            Ok(())
                        }))).unwrap();
                        Ok(())
                    }).map_err(|_| {});
                    TaskExecutor::current().spawn_local(Box::new(client.then(move |_| {
                        println!("Connection {} proxy closed.", cnt);
                        Ok(())
                    }))).unwrap();
                }

                let _ = mles_tx.send(mles_message.clone()).wait().map_err(|err| {
                    println!("Error {}", err);
                    Error::new(ErrorKind::Other, err)
                });
                Ok(())
            });

            let ws_writer = ws_rx.fold(sink, |mut sink, msg| {
                let msg = Message::binary(msg);
                let _ = sink.start_send(msg).map_err(|err| {
                    Error::new(ErrorKind::Other, err)
                });
                let _ = sink.poll_complete().map_err(|err| {
                    Error::new(ErrorKind::Other, err)
                });
                Ok(sink)
            });
            let connection = ws_reader.map(|_| ()).map_err(|_| ())
                                      .select(ws_writer.map(|_| ()).map_err(|_| ()));

            TaskExecutor::current().spawn_local(Box::new(connection.then(move |_| {
                println!("Connection {} closed.", cnt);
                Ok(())
            }))).unwrap();

            Ok(())
        });
        TaskExecutor::current().spawn_local(Box::new(accept.then(move |_| {
            Ok(())
        }))).unwrap();

        //keep accepting connections
        Ok(())
    }).map_err(|_| {});

    runtime.spawn(srv);

    match runtime.run() {
        Ok(_) => {}
        Err(err) => {
            println!("Error: {}", err);
        }
    };
}
