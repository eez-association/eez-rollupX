// Wire-only transcript checker, extracted unchanged from the generator
// workspace. This is development tooling, not a runtime dependency.
use std::collections::HashMap;

use alloy_primitives::{Address, B256, Signature};
use eez_control_rpc::v2::{ClientFrame, ServerFrame, prover_client::ProverClient, server_frame};
use prost::Message as _;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
pub struct Bindings {
    sessions: HashMap<Vec<u8>, Vec<u8>>,
    epochs: HashMap<(Vec<u8>, u64), u64>,
}

impl Bindings {
    pub fn request(&self, mut frame: ClientFrame) -> ClientFrame {
        // Unissued IDs/epochs are literal negative-test inputs. Do not repair them.
        frame.epoch = *self
            .epochs
            .get(&(frame.session_id.clone(), frame.epoch))
            .unwrap_or(&frame.epoch);
        if let Some(id) = self.sessions.get(&frame.session_id) {
            frame.session_id.clone_from(id);
        }
        frame
    }

    pub fn response(
        &mut self,
        expected: &ServerFrame,
        mut actual: ServerFrame,
        attester: Address,
    ) -> Result<ServerFrame, Error> {
        if matches!(actual.kind, Some(server_frame::Kind::Ready(_))) {
            if actual.session_id.len() != 32 || actual.epoch == 0 || expected.session_id.len() != 32
            {
                return Err("invalid Ready binding".into());
            }
            if let Some(id) = self.sessions.get(&expected.session_id) {
                if id != &actual.session_id {
                    return Err("session changed during recovery".into());
                }
            } else {
                if self.sessions.values().any(|id| id == &actual.session_id) {
                    return Err("session reused across Composers".into());
                }
                self.sessions
                    .insert(expected.session_id.clone(), actual.session_id.clone());
            }
            let key = (expected.session_id.clone(), expected.epoch);
            if let Some(epoch) = self.epochs.get(&key) {
                if *epoch != actual.epoch {
                    return Err("epoch changed unexpectedly".into());
                }
            } else {
                if self
                    .epochs
                    .iter()
                    .any(|((id, _), epoch)| id == &expected.session_id && *epoch >= actual.epoch)
                {
                    return Err("control operation did not advance the epoch".into());
                }
                self.epochs.insert(key, actual.epoch);
            }
        }
        if !expected.session_id.is_empty() {
            if self.sessions.get(&expected.session_id) != Some(&actual.session_id)
                || self
                    .epochs
                    .get(&(expected.session_id.clone(), expected.epoch))
                    != Some(&actual.epoch)
            {
                return Err("response crossed session/epoch binding".into());
            }
            actual.session_id.clone_from(&expected.session_id);
            actual.epoch = expected.epoch;
        }
        let mut expected = expected.clone();
        if let Some(server_frame::Kind::Proof(proof)) = &actual.kind {
            let Some(server_frame::Kind::Proof(wanted)) = &mut expected.kind else {
                return Err("unsolicited Proof".into());
            };
            let digest = B256::try_from(wanted.public_inputs_hash.as_slice())?;
            if Signature::try_from(proof.proof.as_slice())?.recover_address_from_prehash(&digest)?
                != attester
            {
                return Err(
                    "proof does not recover configured attester over expected digest".into(),
                );
            }
            // Keep the observed valid nonce for recording; replay accepts any valid nonce.
            wanted.proof.clone_from(&proof.proof);
        }
        for frame in [&mut actual, &mut expected] {
            match &mut frame.kind {
                Some(server_frame::Kind::Validated(v)) => v.reused = false,
                Some(server_frame::Kind::Rejected(r)) => r.message.clear(),
                _ => {}
            }
        }
        if actual != expected {
            return Err(
                format!("response differs: expected {expected:?}; actual {actual:?}").into(),
            );
        }
        Ok(actual)
    }
}

pub async fn replay(vector: &Value, endpoint: &str) -> Result<(), Error> {
    if vector["format"] != 2 {
        return Err("unsupported vector format".into());
    }
    let attester: Address = vector["oracle"]["attester"]
        .as_str()
        .ok_or("missing attester")?
        .parse()?;
    let events = vector["events"].as_array().ok_or("missing events")?;
    if events.is_empty() {
        return Err("empty transcript".into());
    }
    let mut streams = HashMap::new();
    let mut bindings = Bindings::default();
    let mut client = ProverClient::connect(endpoint.to_owned())
        .await?
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
        .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    for (index, row) in events.iter().enumerate() {
        let transport = row["transport"].as_u64().ok_or("missing transport")?;
        match row["direction"].as_str().ok_or("missing direction")? {
            "client" => {
                let bytes = hex::decode(row["protobuf_hex"].as_str().ok_or("missing frame")?)?;
                let frame = bindings.request(ClientFrame::decode(bytes.as_slice())?);
                if let std::collections::hash_map::Entry::Vacant(entry) = streams.entry(transport) {
                    let (tx, rx) = mpsc::channel(16);
                    tx.send(frame).await?;
                    let replies = client
                        .prove_stream(ReceiverStream::new(rx))
                        .await?
                        .into_inner();
                    entry.insert((Some(tx), replies));
                } else {
                    streams[&transport]
                        .0
                        .as_ref()
                        .ok_or("half-closed sender")?
                        .send(frame)
                        .await?;
                }
            }
            "server" => {
                let bytes = hex::decode(row["protobuf_hex"].as_str().ok_or("missing frame")?)?;
                let expected = ServerFrame::decode(bytes.as_slice())?;
                let actual = streams
                    .get_mut(&transport)
                    .ok_or("unknown stream")?
                    .1
                    .message()
                    .await?
                    .ok_or("premature EOF")?;
                bindings
                    .response(&expected, actual, attester)
                    .map_err(|error| format!("event {index} ({}): {error}", row["check"]))?;
            }
            "half_close" => {
                streams.get_mut(&transport).ok_or("unknown stream")?.0 = None;
            }
            "disconnect" => {
                streams.remove(&transport).ok_or("unknown stream")?;
            }
            "rpc_open_error" => {
                let (tx, rx) = mpsc::channel::<ClientFrame>(1);
                let result = client.prove_stream(ReceiverStream::new(rx)).await;
                drop(tx);
                match result {
                    Err(status)
                        if i64::from(i32::from(status.code()))
                            == row["code"].as_i64().ok_or("missing code")? => {}
                    _ => return Err(format!("event {index}: expected RPC admission error").into()),
                }
            }
            "rpc_error" => {
                match streams
                    .get_mut(&transport)
                    .ok_or("unknown stream")?
                    .1
                    .message()
                    .await
                {
                    Err(status)
                        if i64::from(i32::from(status.code()))
                            == row["code"].as_i64().ok_or("missing code")? => {}
                    _ => return Err(format!("event {index}: expected terminal RPC error").into()),
                }
            }
            "eof" => {
                let mut stream = streams.remove(&transport).ok_or("unknown stream")?;
                if stream.1.message().await?.is_some() {
                    return Err("unsolicited response after completion".into());
                }
            }
            other => return Err(format!("unknown event {other}").into()),
        }
    }
    if !streams.is_empty() {
        return Err("transcript left open streams".into());
    }
    Ok(())
}
