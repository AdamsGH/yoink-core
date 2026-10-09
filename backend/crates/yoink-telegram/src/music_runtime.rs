//! Native Music bridge. Callers supply observed Telegram identities, never payload actor IDs.
use std::{sync::Arc, time::Duration};
use teloxide::{
    Bot,
    types::{InlineQuery, Message, MessageEntityKind},
};
use tokio_util::sync::CancellationToken;
use yoink_identity::IdentityStore;
use yoink_music::*;
use yoink_storage::{Store, StoreError, TextEffect, TextIdentity, TextOutcome};

#[derive(Clone, Copy)]
pub struct MusicTransportBudget {
    pub timeout: Duration,
    pub response_bytes: usize,
}
#[derive(Clone)]
pub struct NativeMusicDelivery {
    bot: Bot,
    bot_id: i64,
    budget: MusicTransportBudget,
}
impl NativeMusicDelivery {
    pub fn new(bot: Bot, bot_id: i64, budget: MusicTransportBudget) -> Result<Self, MusicError> {
        if bot_id <= 0
            || budget.timeout.is_zero()
            || budget.timeout > Duration::from_secs(60)
            || !(1024..=crate::text::MAX_TEXT_RESPONSE_BYTES).contains(&budget.response_bytes)
        {
            return Err(MusicError::InvalidInput);
        }
        Ok(Self {
            bot,
            bot_id,
            budget,
        })
    }
    fn request(&self, method: &str) -> reqwest::RequestBuilder {
        let mut endpoint = self.bot.api_url();
        endpoint.set_path(&format!("/bot{}/{method}", self.bot.token()));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        self.bot.client().post(endpoint)
    }
    async fn receive(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, MusicError> {
        tokio::time::timeout(self.budget.timeout, async {
            let mut response = request
                .send()
                .await
                .map_err(|_| MusicError::DeliveryUnknown)?;
            if response
                .content_length()
                .is_some_and(|n| n > self.budget.response_bytes as u64)
            {
                return Err(MusicError::DeliveryUnknown);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| MusicError::DeliveryUnknown)?
            {
                if bytes.len().saturating_add(chunk.len()) > self.budget.response_bytes {
                    return Err(MusicError::DeliveryUnknown);
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| MusicError::DeliveryUnknown)?;
            if value["ok"] == false
                && value["error_code"]
                    .as_u64()
                    .is_some_and(|c| (400..500).contains(&c))
            {
                return Err(MusicError::DeliveryRejected);
            }
            if !response.status().is_success() || value["ok"] != true {
                return Err(MusicError::DeliveryUnknown);
            }
            Ok(value["result"].clone())
        })
        .await
        .unwrap_or(Err(MusicError::DeliveryUnknown))
    }
    fn receipt(
        &self,
        value: serde_json::Value,
        target: AudioTarget,
    ) -> Result<Message, MusicError> {
        let message: Message =
            serde_json::from_value(value).map_err(|_| MusicError::DeliveryUnknown)?;
        if target.bot_id != self.bot_id
            || message.id.0 <= 0
            || message.id.0 == target.incoming_message_id
            || message.chat.id.0 != target.chat_id
            || message.thread_id.map(|t| t.0.0) != target.topic_id
            || !message
                .from
                .as_ref()
                .is_some_and(|u| u.is_bot && u.id.0 == self.bot_id as u64)
            || !(if target.chat_id > 0 {
                message.chat.is_private()
            } else {
                message.chat.is_group() || message.chat.is_supergroup()
            })
        {
            return Err(MusicError::DeliveryUnknown);
        }
        Ok(message)
    }
    async fn card(&self, target: AudioTarget, card: &MusicCard) -> Result<i32, MusicError> {
        let payload = serde_json::json!({"chat_id":target.chat_id,"message_thread_id":target.topic_id,
            "reply_parameters":{"message_id":target.incoming_message_id},"text":card.text,
            "entities":entities(card),"link_preview_options":preview(card)});
        let message = self.receipt(
            self.receive(self.request("sendMessage").json(&payload))
                .await?,
            target,
        )?;
        if message.text() != Some(card.text.as_str()) {
            return Err(MusicError::DeliveryUnknown);
        }
        Ok(message.id.0)
    }
}
fn entities(card: &MusicCard) -> Vec<serde_json::Value> {
    card.entities.iter().map(|e| {
        serde_json::json!({"type":"text_link","offset":e.offset,"length":e.length,"url":e.url})
    }).collect()
}
fn preview(card: &MusicCard) -> serde_json::Value {
    match &card.thumbnail_url {
        Some(url) => {
            serde_json::json!({"url":url,"show_above_text":true,"prefer_large_media":true})
        }
        None => serde_json::json!({"is_disabled":true}),
    }
}
fn store_error(error: StoreError) -> MusicError {
    match error {
        StoreError::Conflict => MusicError::Conflict,
        _ => MusicError::Persistence,
    }
}
fn observation_identity(message: &Message, actor: i64, bot: i64) -> TextIdentity {
    TextIdentity {
        bot,
        chat: message.chat.id.0,
        topic: message.thread_id.map(|t| t.0.0),
        message: message.id.0,
        actor,
        item: None,
        edit: false,
        text: message
            .text()
            .or_else(|| message.caption())
            .unwrap_or_default()
            .to_owned(),
        payload: serde_json::json!({"kind":"music_observed_card","bind_item":false,"observation":{
            "date":message.date,"chat":message.chat,"from":message.from,"via_bot":message.via_bot,
            "sender_chat":message.sender_chat,"text":message.text(),"caption":message.caption(),
            "entities":message.entities(),"caption_entities":message.caption_entities(),
            "forward_origin":message.forward_origin()
        }}),
    }
}
fn observation_token(identity: &TextIdentity) -> String {
    format!(
        "music:observation:{}:{}:{:?}:{}",
        identity.bot, identity.chat, identity.topic, identity.message
    )
}
fn observation_value(
    effect: TextEffect,
    observed: &TextIdentity,
) -> Result<serde_json::Value, MusicError> {
    let mut stored = effect.identity.ok_or(MusicError::Conflict)?;
    let resolution = stored
        .payload
        .as_object_mut()
        .ok_or(MusicError::Persistence)?
        .remove("resolution")
        .ok_or(MusicError::Persistence)?;
    if &stored != observed {
        return Err(MusicError::Conflict);
    }
    if effect.outcome != TextOutcome::Prepared {
        return Err(MusicError::DeliveryUnknown);
    }
    Ok(resolution)
}
async fn observation_snapshot(
    connection: &turso::Connection,
    effect: TextEffect,
    observed: &TextIdentity,
) -> Result<Vec<Resolution>, MusicError> {
    let resolution = observation_value(effect, observed)?;
    if let Some(parts) = resolution.get("parts") {
        // The service validates max_links_per_command in 1..=10.
        let count = parts
            .as_u64()
            .filter(|count| (1..=10).contains(count))
            .ok_or(MusicError::Persistence)?;
        let token = observation_token(observed);
        let mut results = Vec::new();
        for index in 0..count {
            let effect = Store::text_effect_on(connection, &format!("{token}:result:{index}"))
                .await
                .map_err(store_error)?
                .ok_or(MusicError::Persistence)?;
            results.push(
                serde_json::from_value(observation_value(effect, observed)?)
                    .map_err(|_| MusicError::Persistence)?,
            );
        }
        return Ok(results);
    }
    // Earlier own-card intents stored one result. New observed-message intents
    // preserve the complete ordered result list, including passive multi-links.
    if resolution.is_array() {
        serde_json::from_value(resolution).map_err(|_| MusicError::Persistence)
    } else {
        serde_json::from_value(resolution)
            .map(|result| vec![result])
            .map_err(|_| MusicError::Persistence)
    }
}
async fn prepare_observation_on(
    connection: &turso::Connection,
    identity: &TextIdentity,
    resolution: Vec<Resolution>,
) -> Result<Vec<Resolution>, MusicError> {
    let token = observation_token(identity);
    if let Some(effect) = Store::text_effect_on(connection, &token)
        .await
        .map_err(store_error)?
    {
        return observation_snapshot(connection, effect, identity).await;
    }
    let mut prepared = identity.clone();
    prepared.payload["resolution"] =
        serde_json::to_value(&resolution).map_err(|_| MusicError::Persistence)?;
    if !prepared.valid() {
        if !(1..=10).contains(&resolution.len()) {
            return Err(MusicError::InvalidInput);
        }
        // Keep each result within the shared payload cap. The manifest and
        // every result commit together in the caller's observation transaction.
        prepared.payload["resolution"] = serde_json::json!({"parts":resolution.len()});
        for (index, result) in resolution.iter().enumerate() {
            let mut part = identity.clone();
            part.payload["resolution"] =
                serde_json::to_value(result).map_err(|_| MusicError::Persistence)?;
            Store::prepare_text_on(connection, &format!("{token}:result:{index}"), &part)
                .await
                .map_err(store_error)?;
        }
    }
    Store::prepare_text_on(connection, &token, &prepared)
        .await
        .map_err(store_error)?;
    Ok(resolution)
}
impl MusicAudioDelivery for NativeMusicDelivery {
    fn send(
        &self,
        target: AudioTarget,
        title: String,
        artist: Option<String>,
        caption: MusicCard,
        artifact: MusicAudioArtifact,
    ) -> MusicFuture<'_, AudioReceipt> {
        Box::pin(async move {
            if target.bot_id != self.bot_id {
                return Err(MusicError::DeliveryRejected);
            }
            let mut form = reqwest::multipart::Form::new()
                .text("chat_id", target.chat_id.to_string())
                .text(
                    "reply_parameters",
                    serde_json::json!({"message_id":target.incoming_message_id}).to_string(),
                )
                .text("title", title)
                .text("caption", caption.text.clone())
                .text(
                    "caption_entities",
                    serde_json::to_string(&entities(&caption))
                        .map_err(|_| MusicError::DeliveryRejected)?,
                );
            if let Some(topic) = target.topic_id {
                form = form.text("message_thread_id", topic.to_string());
            }
            if let Some(artist) = artist {
                form = form.text("performer", artist);
            }
            form = match artifact {
                MusicAudioArtifact::Cached(id) => form.text("audio", id),
                MusicAudioArtifact::Upload(artifact) => {
                    let size = artifact.size_bytes;
                    let file = tokio::fs::File::from_std(artifact.into_file());
                    let part = reqwest::multipart::Part::stream_with_length(
                        reqwest::Body::from(file),
                        size,
                    )
                    .file_name("music.mp3")
                    .mime_str("audio/mpeg")
                    .map_err(|_| MusicError::DeliveryRejected)?;
                    form.part("audio", part)
                }
            };
            let message = self.receipt(
                self.receive(self.request("sendAudio").multipart(form))
                    .await?,
                target,
            )?;
            let audio = message.audio().ok_or(MusicError::DeliveryUnknown)?;
            Ok(AudioReceipt {
                message_id: message.id.0,
                file_id: Some(audio.file.id.0.clone()),
            })
        })
    }
}

#[derive(Clone)]
pub struct MusicRuntime {
    service: MusicService,
    persistence: Arc<NativeMusicPersistence>,
    identities: Arc<IdentityStore>,
    delivery: Arc<NativeMusicDelivery>,
    groups: bool,
    stop: CancellationToken,
    handlers: crate::insight_lifecycle::Handlers,
}
#[derive(Debug, PartialEq, Eq)]
pub enum MusicInlineOutcome {
    Unsupported,
    Answered,
}
impl MusicRuntime {
    pub fn new(
        service: MusicService,
        persistence: Arc<NativeMusicPersistence>,
        identities: Arc<IdentityStore>,
        delivery: Arc<NativeMusicDelivery>,
        groups: bool,
    ) -> Result<Self, MusicError> {
        if persistence.database.path() != persistence.store.database().path() {
            return Err(MusicError::Persistence);
        }
        Ok(Self {
            service,
            persistence,
            identities,
            delivery,
            groups,
            stop: CancellationToken::new(),
            handlers: crate::insight_lifecycle::Handlers::new(),
        })
    }
    pub fn stop_admission(&self) {
        self.handlers.close();
    }
    pub fn cancel_all(&self) {
        self.stop_admission();
        self.stop.cancel();
        self.service.cancel_all();
    }
    pub async fn drain_handlers(&self) -> Result<(), MusicError> {
        self.cancel_all();
        self.handlers.drain().await;
        // Includes dropped resolution owners, provider futures and real executor reap.
        self.service.shutdown().await
    }
    async fn admission<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, MusicError>>,
    ) -> Result<T, MusicError> {
        tokio::select! {
            _ = self.stop.cancelled() => Err(MusicError::Cancelled),
            result = tokio::time::timeout(self.service.persistence_timeout(),future) => result.map_err(|_|MusicError::Persistence)?,
        }
    }
    async fn require(&self, actor: MusicActor) -> Result<(), MusicError> {
        if self.stop.is_cancelled() {
            return Err(MusicError::Cancelled);
        }
        if let Some((chat, _)) = actor.group
            && (!self.groups
                || !self
                    .admission(async {
                        self.identities
                            .membership_eligible(self.delivery.bot_id, chat, actor.user_id)
                            .await
                            .map_err(|_| MusicError::Persistence)
                    })
                    .await?)
        {
            return Err(MusicError::Denied);
        }
        if !self
            .admission(NativeMusicAuthority(self.identities.clone()).allowed(actor))
            .await?
        {
            return Err(MusicError::Denied);
        }
        Ok(())
    }
    /// Returns false only for non-Music commands, including another bot's command.
    pub async fn handle_command(
        &self,
        message: &Message,
        actor: i64,
        bot_username: &str,
    ) -> Result<bool, MusicError> {
        let text = message.text().unwrap_or_default();
        let head = text.split_whitespace().next().unwrap_or_default();
        let (command, addressed) = head
            .split_once('@')
            .map_or((head, None), |(c, b)| (c, Some(b)));
        if !(command.eq_ignore_ascii_case("/music") || command.eq_ignore_ascii_case("/song"))
            || addressed.is_some_and(|b| !b.eq_ignore_ascii_case(bot_username))
        {
            return Ok(false);
        }
        self.handle_message(message, actor, true, false).await
    }
    /// Passive links and cards posted through this bot. Time and age limit are supplied
    /// by the native dispatcher, not guessed from card text or local configuration.
    pub async fn handle_observed_message(
        &self,
        message: &Message,
        actor: i64,
        observed_at: chrono::DateTime<chrono::Utc>,
        max_age: Duration,
    ) -> Result<bool, MusicError> {
        let age = observed_at
            .signed_duration_since(message.date)
            .to_std()
            .ok();
        if max_age.is_zero() || age.is_none_or(|age| age > max_age) {
            return Ok(false);
        }
        let _handler = self.handlers.admit().ok_or(MusicError::Cancelled)?;
        if !message
            .from
            .as_ref()
            .is_some_and(|u| !u.is_bot && actor > 0 && u.id.0 == actor as u64)
            || message.sender_chat.is_some()
        {
            return Err(MusicError::Denied);
        }
        // Check even if changed provenance or source would now fail classification.
        self.observed_snapshot(&observation_identity(message, actor, self.delivery.bot_id))
            .await?;
        let via_card = message
            .via_bot
            .as_ref()
            .is_some_and(|b| b.is_bot && b.id.0 == self.delivery.bot_id as u64);
        let text = message
            .text()
            .or_else(|| message.caption())
            .unwrap_or_default();
        if text.starts_with('/') {
            return Ok(false);
        }
        let hidden = hidden_links(message);
        if (via_card && extract_links("", &hidden, 1).is_empty())
            || extract_links(text, &hidden, 1).is_empty()
        {
            return Ok(false);
        }
        self.handle_message(message, actor, false, via_card).await
    }
    async fn observed_snapshot(
        &self,
        identity: &TextIdentity,
    ) -> Result<Option<Vec<Resolution>>, MusicError> {
        self.admission(async {
            let connection = self
                .persistence
                .database
                .connection()
                .await
                .map_err(|_| MusicError::Persistence)?;
            match Store::text_effect_on(&*connection, &observation_token(identity))
                .await
                .map_err(store_error)?
            {
                Some(effect) => observation_snapshot(&connection, effect, identity)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        })
        .await
    }
    async fn prepare_observation(
        &self,
        identity: &TextIdentity,
        resolution: Vec<Resolution>,
    ) -> Result<Vec<Resolution>, MusicError> {
        self.admission(async {
            let mut connection = self
                .persistence
                .database
                .connection()
                .await
                .map_err(|_| MusicError::Persistence)?;
            let transaction = yoink_db::Transaction::new(
                &mut connection,
                turso::transaction::TransactionBehavior::Immediate,
            )
            .await
            .map_err(|_| MusicError::Persistence)?;
            let snapshot = prepare_observation_on(&transaction, identity, resolution).await?;
            transaction
                .commit()
                .await
                .map_err(|_| MusicError::Persistence)?;
            Ok(snapshot)
        })
        .await
    }
    async fn handle_message(
        &self,
        message: &Message,
        actor: i64,
        command: bool,
        via_card: bool,
    ) -> Result<bool, MusicError> {
        let text = message
            .text()
            .or_else(|| message.caption())
            .unwrap_or_default();
        let _handler = self.handlers.admit().ok_or(MusicError::Cancelled)?;
        if !message
            .from
            .as_ref()
            .is_some_and(|u| !u.is_bot && u.id.0 == actor as u64)
            || actor <= 0
            || message.sender_chat.is_some()
            || message.id.0 <= 0
            || !(message.chat.is_private()
                || message.chat.is_group()
                || message.chat.is_supergroup())
            || (message.chat.id.0 > 0
                && (message.chat.id.0 != actor || message.thread_id.is_some()))
        {
            return Err(MusicError::Denied);
        }
        let target = AudioTarget {
            bot_id: self.delivery.bot_id,
            chat_id: message.chat.id.0,
            topic_id: message.thread_id.map(|t| t.0.0),
            incoming_message_id: message.id.0,
        };
        let actor = MusicActor {
            user_id: actor,
            group: (target.chat_id < 0).then_some((target.chat_id, target.topic_id.map(i64::from))),
        };
        self.require(actor).await?;
        let mut input = text.to_owned();
        let mut hidden = hidden_links(message);
        if command && let Some(reply) = message.reply_to_message() {
            input.push('\n');
            input.push_str(reply.text().or_else(|| reply.caption()).unwrap_or_default());
            hidden.extend(hidden_links(reply));
        }
        let cancel = self.stop.child_token();
        if !command {
            // Entity-only cards have several provider URLs for one track.
            let visible = extract_links(text, &[], 1);
            if via_card || visible.is_empty() {
                input.clear();
                hidden = extract_links("", &hidden, 1)
                    .into_iter()
                    .map(|l| l.url.as_url().to_string())
                    .collect();
            } else {
                hidden.clear();
            }
        }
        let results = if !command {
            let identity = observation_identity(message, actor.user_id, self.delivery.bot_id);
            if let Some(snapshot) = self.observed_snapshot(&identity).await? {
                snapshot
            } else {
                let resolved = self
                    .service
                    .command(actor, &input, &hidden, cancel.clone())
                    .await?;
                if resolved.is_empty() {
                    return Ok(false);
                }
                // Both observed branches commit the same immutable message intent
                // before any card/audio journal admission. A preliminary read is
                // only a replay optimization, never the arbitration boundary.
                self.prepare_observation(&identity, resolved).await?
            }
        } else {
            self.service
                .command(actor, &input, &hidden, cancel.clone())
                .await?
        };
        if !command && results.is_empty() {
            return Ok(false);
        }
        if via_card {
            for resolution in &results {
                self.deliver_resolution_audio(resolution, message, actor, target, cancel.clone())
                    .await?;
            }
            return Ok(true);
        }
        for (index, resolution) in results.into_iter().enumerate() {
            let mut card = MusicCard::from_resolution(&resolution);
            card.add_requester(
                actor.user_id,
                &message.from.as_ref().ok_or(MusicError::Denied)?.full_name(),
            );
            let token = format!(
                "music:card:{}:{}:{:?}:{}:{index}",
                target.bot_id, target.chat_id, target.topic_id, target.incoming_message_id
            );
            let identity = TextIdentity {
                bot: target.bot_id,
                actor: actor.user_id,
                chat: target.chat_id,
                topic: target.topic_id,
                message: target.incoming_message_id,
                item: None,
                edit: false,
                text: card.text.clone(),
                payload: serde_json::json!({"kind":"music_card","bind_item":false,"resolution":resolution,"entities":entities(&card)}),
            };
            self.require(actor).await?;
            let confirmed = if self
                .admission(async {
                    self.persistence
                        .store
                        .begin_text(&token, &identity)
                        .await
                        .map_err(store_error)
                })
                .await?
            {
                self.require(actor).await?;
                match tokio::select! { _=cancel.cancelled()=>Err(MusicError::DeliveryUnknown),r=self.delivery.card(target,&card)=>r }
                {
                    Ok(id) => {
                        let _ = tokio::time::timeout(
                            self.service.persistence_timeout(),
                            self.persistence.store.confirm_text(&token, id),
                        )
                        .await;
                        true
                    }
                    Err(e) => {
                        let _ = tokio::time::timeout(
                            self.service.persistence_timeout(),
                            self.persistence.store.unknown_text(&token),
                        )
                        .await;
                        return Err(e);
                    }
                }
            } else {
                self.admission(async {
                    self.persistence
                        .store
                        .text_effect(&token)
                        .await
                        .map_err(store_error)
                })
                .await?
                .is_some_and(|e| {
                    e.identity.as_ref() == Some(&identity)
                        && matches!(e.outcome, TextOutcome::Confirmed(_))
                })
            };
            if confirmed {
                self.deliver_resolution_audio(&resolution, message, actor, target, cancel.clone())
                    .await?;
            }
        }
        Ok(true)
    }
    async fn deliver_resolution_audio(
        &self,
        resolution: &Resolution,
        message: &Message,
        actor: MusicActor,
        target: AudioTarget,
        cancel: CancellationToken,
    ) -> Result<(), MusicError> {
        if let Resolution::Track(track) = resolution {
            let outcome = self
                .send_audio_admitted(
                    AudioCommand {
                        actor,
                        target,
                        track,
                        caption: MusicCard::audio_caption(
                            actor.user_id,
                            &message.from.as_ref().ok_or(MusicError::Denied)?.full_name(),
                        ),
                    },
                    cancel,
                )
                .await?;
            match outcome {
                AudioOutcome::Unknown => return Err(MusicError::DeliveryUnknown),
                AudioOutcome::Rejected => return Err(MusicError::DeliveryRejected),
                AudioOutcome::Confirmed(_) | AudioOutcome::Disabled => {}
            }
        }
        Ok(())
    }
    /// Trusted callers supply a caption containing the observed requester's mention.
    /// Command handling already does this. Cancellation is owned by cancel_all.
    pub async fn send_audio(&self, command: AudioCommand<'_>) -> Result<AudioOutcome, MusicError> {
        let _handler = self.handlers.admit().ok_or(MusicError::Cancelled)?;
        self.send_audio_admitted(command, self.stop.child_token())
            .await
    }
    async fn send_audio_admitted(
        &self,
        command: AudioCommand<'_>,
        cancel: CancellationToken,
    ) -> Result<AudioOutcome, MusicError> {
        self.require(command.actor).await?;
        if command.target.bot_id != self.delivery.bot_id {
            return Err(MusicError::Denied);
        }
        let delivery = Arc::new(AuthorizedAudio {
            runtime: self.clone(),
            actor: command.actor,
        });
        self.service
            .send_audio(command, self.persistence.clone(), delivery, cancel)
            .await
    }
    /// Unsupported queries are left untouched for the coordinator's central fallback.
    pub async fn handle_inline(
        &self,
        query: &InlineQuery,
    ) -> Result<MusicInlineOutcome, MusicError> {
        let links = extract_links(&query.query, &[], 10);
        if links.is_empty() {
            return Ok(MusicInlineOutcome::Unsupported);
        }
        let _handler = self.handlers.admit().ok_or(MusicError::Cancelled)?;
        let actor = MusicActor {
            user_id: i64::try_from(query.from.id.0).map_err(|_| MusicError::Denied)?,
            group: None,
        };
        if query.from.is_bot || query.id.0.is_empty() || query.id.0.len() > 256 {
            return Err(MusicError::Denied);
        }
        self.require(actor).await?;
        let answer = self
            .service
            .inline(actor.user_id, &query.query, self.stop.child_token())
            .await?;
        if answer.results.is_empty() && links.iter().all(|l| l.provider == MusicProvider::Youtube) {
            return Ok(MusicInlineOutcome::Unsupported);
        }
        let preference = self
            .admission(async {
                self.identities
                    .language_preference(actor.user_id)
                    .await
                    .map_err(|_| MusicError::Persistence)
            })
            .await?;
        let language = yoink_i18n::Language::from_code(
            preference
                .as_ref()
                .map(|p| p.code.as_str())
                .or(query.from.language_code.as_deref()),
        );
        let share = if language == yoink_i18n::Language::Russian {
            "Поделиться"
        } else {
            "Share"
        };
        let results: Vec<_> = answer.results.into_iter().enumerate().map(|(i,mut card)| {
            card.add_requester(actor.user_id, &query.from.full_name());
            serde_json::json!({
                "type":"article", "id":i.to_string(), "title":card.title,
                "thumbnail_url":card.thumbnail_url,
                "reply_markup":{"inline_keyboard":[[{"text":share,"switch_inline_query":query.query}]]},
                "input_message_content":{
                    "message_text":card.text, "entities":entities(&card),
                    "link_preview_options":preview(&card)
                }
            })
        }).collect();
        self.require(actor).await?;
        let payload = serde_json::json!({"inline_query_id":query.id.0,"results":results,"cache_time":0,"is_personal":true,
            "button":{"text":error_message(MusicError::InvalidInput,if language == yoink_i18n::Language::Russian { "ru" } else { "en" }),"start_parameter":"music_help"}});
        let result = tokio::select! { _=self.stop.cancelled()=>return Err(MusicError::Cancelled),r=self.delivery.receive(self.delivery.request("answerInlineQuery").json(&payload))=>r? };
        if result != true {
            return Err(MusicError::DeliveryUnknown);
        }
        Ok(MusicInlineOutcome::Answered)
    }
}
fn hidden_links(message: &Message) -> Vec<String> {
    message
        .entities()
        .into_iter()
        .flatten()
        .chain(message.caption_entities().into_iter().flatten())
        .filter_map(|e| match &e.kind {
            MessageEntityKind::TextLink { url } => Some(url.to_string()),
            _ => None,
        })
        .collect()
}
struct AuthorizedAudio {
    runtime: MusicRuntime,
    actor: MusicActor,
}
impl MusicAudioDelivery for AuthorizedAudio {
    fn send(
        &self,
        target: AudioTarget,
        title: String,
        artist: Option<String>,
        caption: MusicCard,
        artifact: MusicAudioArtifact,
    ) -> MusicFuture<'_, AudioReceipt> {
        Box::pin(async move {
            self.runtime.require(self.actor).await?;
            self.runtime
                .delivery
                .send(target, title, artist, caption, artifact)
                .await
        })
    }
}
#[cfg(test)]
#[path = "music_runtime_tests.rs"]
mod tests;
