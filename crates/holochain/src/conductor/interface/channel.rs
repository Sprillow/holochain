use crate::conductor::{api::*, interface::interface::*};
use crate::core::signal::Signal;
//use async_trait::async_trait;
//use tracing::*;
use super::error::{InterfaceError, InterfaceResult};
use holochain_serialized_bytes::SerializedBytes;
use holochain_websocket::{
    websocket_bind, WebsocketConfig, WebsocketMessage, WebsocketReceiver, WebsocketSender,
};
use std::convert::{TryFrom};
use std::sync::Arc;
use tokio::stream::StreamExt;
use tokio::sync::broadcast;
use tracing::*;
use url2::url2;

/// A trivial Interface, used for proof of concept only,
/// which is driven externally by a channel in order to
/// interact with a AppInterfaceApi
pub fn create_demo_channel_interface<A: AppInterfaceApi>(
    api: A,
) -> (
    futures::channel::mpsc::Sender<(SerializedBytes, ExternalSideResponder)>,
    tokio::task::JoinHandle<()>,
) {
    let (send_sig, _recv_sig) = futures::channel::mpsc::channel(1);
    let (send_req, recv_req) = futures::channel::mpsc::channel(1);

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Stub;
    holochain_serialized_bytes::holochain_serial!(Stub);

    let (_chan_sig_send, chan_req_recv): (
        ConductorSideSignalSender<Stub>, // stub impl signals
        ConductorSideRequestReceiver<AppRequest, AppResponse>,
    ) = create_interface_channel(send_sig, recv_req);

    let join_handle = attach_external_conductor_api(api, chan_req_recv);

    (send_req, join_handle)
}

/// Create an Admin Interface, which only receives AdminRequest messages
/// from the external client
pub async fn create_admin_interface<A: InterfaceApi>(api: A, port: u16) -> InterfaceResult<()> {
    trace!("Initializing Admin interface");
    let mut listener = websocket_bind(
        url2!("ws://127.0.0.1:{}", port),
        Arc::new(WebsocketConfig::default()),
    )
    .await?;
    trace!("LISTENING AT: {}", listener.local_addr());
    let mut listener_handles = Vec::new();
    // TODO there is no way to exit this listner
    // If we remove the interface then we want to kill this lister
    while let Some(maybe_con) = listener.next().await {
        let (_, recv_socket) = maybe_con.await?;
        listener_handles.push(tokio::task::spawn(recv_incoming_admin_msgs(
            // FIXME not sure if clone is correct here
            api.clone(),
            recv_socket,
        )));
    }
    for h in listener_handles {
        h.await?;
    }
    Ok(())
}

/// Create an App Interface, which includes the ability to receive signals
/// from Cells via a broadcast channel
pub async fn create_app_interface<A: InterfaceApi>(
    api: A,
    port: u16,
    signal_broadcaster: broadcast::Sender<Signal>,
) -> InterfaceResult<()> {
    trace!("Initializing App interface");
    let mut listener = websocket_bind(
        url2!("ws://127.0.0.1:{}", port),
        Arc::new(WebsocketConfig::default()),
    )
    .await?;
    trace!("LISTENING AT: {}", listener.local_addr());
    let mut listener_handles = Vec::new();
    // TODO there is no way to exit this listner
    // If we remove the interface then we want to kill this lister
    while let Some(maybe_con) = listener.next().await {
        let (send_socket, recv_socket) = maybe_con.await?;
        let signal_rx = signal_broadcaster.subscribe();
        listener_handles.push(tokio::task::spawn(recv_incoming_msgs_and_outgoing_signals(
            // FIXME not sure if clone is correct here
            api.clone(),
            recv_socket,
            signal_rx,
            send_socket,
        )));
    }
    for h in listener_handles {
        h.await??;
    }
    Ok(())
}

/// Polls for messages coming in from the external client.
/// Used by Admin interface.
async fn recv_incoming_admin_msgs<A: InterfaceApi>(
    api: A,
    mut recv_socket: WebsocketReceiver,
) -> () {
    while let Some(msg) = recv_socket.next().await {
        // FIXME I'm not sure if cloning is the right thing to do here
        if let Err(_todo) = handle_incoming_message(msg, api.clone()).await {
            break;
        }
    }
}

/// Polls for messages coming in from the external client while simultaneously
/// polling for signals being broadcast from the Cells associated with this
/// App interface.
async fn recv_incoming_msgs_and_outgoing_signals<A: InterfaceApi>(
    api: A,
    mut recv_socket: WebsocketReceiver,
    mut signal_rx: broadcast::Receiver<Signal>,
    mut signal_tx: WebsocketSender,
) -> InterfaceResult<()> {
    trace!("CONNECTION: {}", recv_socket.remote_addr());

    loop {
        tokio::select! {
            // If we receive a Signal broadcasted from a Cell, push it out
            // across the interface
            signal = signal_rx.next() => {
                if let Some(signal) = signal {
                    let bytes = SerializedBytes::try_from(
                        signal.map_err(InterfaceError::SignalReceive)?,
                    )?;
                    signal_tx.signal(bytes).await?;
                } else {
                    debug!("Closing interface: signal stream empty");
                    break;
                }
            },

            // If we receive a message from outside, handle it
            msg = recv_socket.next() => {
                if let Some(msg) = msg {
                    // FIXME I'm not sure if cloning is the right thing to do here
                    handle_incoming_message(msg, api.clone()).await?
                } else {
                    debug!("Closing interface: message stream empty");
                    break;
                }
            },
        }
    }

    Ok(())
}

async fn handle_incoming_message<A>(ws_msg: WebsocketMessage, api: A) -> InterfaceResult<()>
where
    A: InterfaceApi,
{
    match ws_msg {
        WebsocketMessage::Request(bytes, respond) => {
            Ok(respond(api.handle_request(bytes).await?).await?)
        }
        // FIXME this will kill this interface, is that what we want?
        WebsocketMessage::Signal(_) => Err(InterfaceError::UnexpectedMessage(
            "Got an unexpected Signal while handing incoming message".to_string(),
        )),
        WebsocketMessage::Close(_) => unimplemented!(),
    }
}

async fn handle_incoming_admin_request(request: AdminRequest) -> InterfaceResult<AdminResponse> {
    Ok(match request {
        _ => AdminResponse::DnaAdded,
    })
}

// TODO: rename AppRequest to AppRequest or something
async fn handle_incoming_app_request(request: AppRequest) -> InterfaceResult<AppResponse> {
    Ok(match request {
        _ => AppResponse::Error {
            debug: "TODO".into(),
        },
    })
}
