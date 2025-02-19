#[cfg(feature = "http")]
use super::Builder;
use super::{
    CreateActionRow,
    CreateAllowedMentions,
    CreateAttachment,
    CreateEmbed,
    ExistingAttachment,
};
#[cfg(feature = "http")]
use crate::constants;
#[cfg(feature = "http")]
use crate::http::CacheHttp;
#[cfg(feature = "http")]
use crate::internal::prelude::*;
use crate::model::prelude::*;
#[cfg(feature = "http")]
use crate::utils::check_overflow;

/// A builder to specify the fields to edit in an existing [`Webhook`]'s message.
///
/// [Discord docs](https://discord.com/developers/docs/resources/webhook#edit-webhook-message)
#[derive(Clone, Debug, Default, Serialize)]
#[must_use]
pub struct EditWebhookMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    embeds: Option<Vec<CreateEmbed>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_mentions: Option<CreateAllowedMentions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) components: Option<Vec<CreateActionRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<ExistingAttachment>>,

    #[serde(skip)]
    thread_id: Option<ChannelId>,
    #[serde(skip)]
    pub(crate) files: Vec<CreateAttachment>,
}

impl EditWebhookMessage {
    /// Equivalent to [`Self::default`].
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(feature = "http")]
    pub(crate) fn check_length(&self) -> Result<()> {
        if let Some(content) = &self.content {
            check_overflow(content.chars().count(), constants::MESSAGE_CODE_LIMIT)
                .map_err(|overflow| Error::Model(ModelError::MessageTooLong(overflow)))?;
        }

        if let Some(embeds) = &self.embeds {
            check_overflow(embeds.len(), constants::EMBED_MAX_COUNT)
                .map_err(|_| Error::Model(ModelError::EmbedAmount))?;
            for embed in embeds {
                embed.check_length()?;
            }
        }

        Ok(())
    }

    /// Set the content of the message.
    ///
    /// **Note**: Message contents must be under 2000 unicode code points.
    #[inline]
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Edits a message within a given thread. If the provided thread Id doesn't belong to the
    /// current webhook, the API will return an error.
    #[inline]
    pub fn in_thread(mut self, thread_id: impl Into<ChannelId>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    /// Adds an embed for the message.
    ///
    /// Embeds from the original message are reset when adding new embeds and must be re-added.
    pub fn add_embed(mut self, embed: CreateEmbed) -> Self {
        self.embeds.get_or_insert(Vec::new()).push(embed);
        self
    }

    /// Adds multiple embeds to the message.
    ///
    /// Embeds from the original message are reset when adding new embeds and must be re-added.
    pub fn add_embeds(mut self, embeds: Vec<CreateEmbed>) -> Self {
        self.embeds.get_or_insert(Vec::new()).extend(embeds);
        self
    }

    /// Sets a single embed to include in the message
    ///
    /// Calling this will overwrite the embed list. To append embeds, call [`Self::add_embed`]
    /// instead.
    pub fn embed(mut self, embed: CreateEmbed) -> Self {
        self.embeds = Some(vec![embed]);
        self
    }

    /// Sets the embeds for the message.
    ///
    /// **Note**: You can only have up to 10 embeds per message.
    ///
    /// Calling this will overwrite the embed list. To append embeds, call [`Self::add_embeds`]
    /// instead.
    pub fn embeds(mut self, embeds: Vec<CreateEmbed>) -> Self {
        self.embeds = Some(embeds);
        self
    }

    /// Set the allowed mentions for the message.
    pub fn allowed_mentions(mut self, allowed_mentions: CreateAllowedMentions) -> Self {
        self.allowed_mentions = Some(allowed_mentions);
        self
    }

    /// Sets the components for this message. Requires an application-owned webhook, meaning either
    /// the webhook's `kind` field is set to [`WebhookType::Application`], or it was created by an
    /// application (and has kind [`WebhookType::Incoming`]).
    ///
    /// [`WebhookType::Application`]: crate::model::webhook::WebhookType
    /// [`WebhookType::Incoming`]: crate::model::webhook::WebhookType
    pub fn components(mut self, components: Vec<CreateActionRow>) -> Self {
        self.components = Some(components);
        self
    }
    super::button_and_select_menu_convenience_methods!(self.components);

    /// Add a new attachment for the message.
    ///
    /// This can be called multiple times.
    ///
    /// If this is called one or more times, existing attachments will reset. To keep them, provide
    /// their IDs to [`Self::keep_existing_attachment`].
    pub fn new_attachment(mut self, attachment: CreateAttachment) -> Self {
        self.files.push(attachment);
        self
    }

    /// Keeps an existing attachment by id.
    ///
    /// To be used after [`Self::new_attachment`] or [`Self::clear_existing_attachments`].
    pub fn keep_existing_attachment(mut self, id: AttachmentId) -> Self {
        self.attachments.get_or_insert_with(Vec::new).push(ExistingAttachment {
            id,
        });
        self
    }

    /// Clears existing attachments.
    ///
    /// In combination with [`Self::keep_existing_attachment`], this can be used to selectively
    /// keep only some existing attachments.
    pub fn clear_existing_attachments(mut self) -> Self {
        self.attachments = Some(Vec::new());
        self
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl Builder for EditWebhookMessage {
    type Context<'ctx> = (WebhookId, &'ctx str, MessageId);
    type Built = Message;

    /// Edits the webhook's message.
    ///
    /// **Note**: Message contents must be under 2000 unicode code points, and embeds must be under
    /// 6000 code points.
    ///
    /// # Errors
    ///
    /// Returns an [`Error::Model`] if the message content is too long.
    ///
    /// May also return an [`Error::Http`] if the content is malformed, the webhook's token is
    /// invalid, or the given message Id does not belong to the webhook.
    ///
    /// Or may return an [`Error::Json`] if there is an error deserialising Discord's response.
    async fn execute(
        mut self,
        cache_http: impl CacheHttp,
        ctx: Self::Context<'_>,
    ) -> Result<Self::Built> {
        self.check_length()?;
        let files = std::mem::take(&mut self.files);
        cache_http
            .http()
            .edit_webhook_message(ctx.0, self.thread_id, ctx.1, ctx.2, &self, files)
            .await
    }
}
