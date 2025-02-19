use std::cmp::Ordering;
use std::fmt;

#[cfg(feature = "model")]
use crate::builder::EditRole;
#[cfg(all(feature = "cache", feature = "model"))]
use crate::cache::Cache;
#[cfg(feature = "model")]
use crate::http::Http;
#[cfg(all(feature = "cache", feature = "model"))]
use crate::internal::prelude::*;
use crate::model::prelude::*;
use crate::model::utils::is_false;

/// Information about a role within a guild. A role represents a set of permissions, and can be
/// attached to one or multiple users. A role has various miscellaneous configurations, such as
/// being assigned a colour. Roles are unique per guild and do not cross over to other guilds in
/// any way, and can have channel-specific permission overrides in addition to guild-level
/// permissions.
///
/// [Discord docs](https://discord.com/developers/docs/topics/permissions#role-object).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Role {
    /// The Id of the role. Can be used to calculate the role's creation date.
    pub id: RoleId,
    /// The Id of the Guild the Role is in.
    #[serde(default)]
    pub guild_id: GuildId,
    /// The colour of the role.
    #[serde(rename = "color")]
    pub colour: Colour,
    /// Indicator of whether the role is pinned above lesser roles.
    ///
    /// In the client, this causes [`Member`]s in the role to be seen above those in roles with a
    /// lower [`Self::position`].
    pub hoist: bool,
    /// Indicator of whether the role is managed by an integration service.
    pub managed: bool,
    /// Indicator of whether the role can be mentioned, similar to mentioning a specific member or
    /// `@everyone`.
    ///
    /// Only members of the role will be notified if a role is mentioned with this set to `true`.
    #[serde(default)]
    pub mentionable: bool,
    /// The name of the role.
    pub name: String,
    /// A set of permissions that the role has been assigned.
    ///
    /// See the [`permissions`] module for more information.
    ///
    /// [`permissions`]: crate::model::permissions
    pub permissions: Permissions,
    /// The role's position in the position list. Roles are considered higher in hierarchy if their
    /// position is higher.
    ///
    /// The `@everyone` role is usually either `-1` or `0`.
    pub position: u16,
    /// The tags this role has. It can be used to determine if this role is a special role in this
    /// guild such as guild subscriber role, or if the role is linked to an [`Integration`] or a
    /// bot.
    ///
    /// [`Integration`]: super::Integration
    #[serde(default)]
    pub tags: RoleTags,
    /// Role icon image hash.
    pub icon: Option<ImageHash>,
    /// Role unicoded image.
    pub unicode_emoji: Option<String>,
}

#[cfg(feature = "model")]
impl Role {
    /// Deletes the role.
    ///
    /// **Note** Requires the [Manage Roles] permission.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] if the current user lacks permission to delete this role.
    ///
    /// [Manage Roles]: Permissions::MANAGE_ROLES
    #[inline]
    pub async fn delete(&mut self, http: impl AsRef<Http>) -> Result<()> {
        http.as_ref().delete_role(self.guild_id, self.id, None).await
    }

    /// Edits a [`Role`], optionally setting its new fields.
    ///
    /// Requires the [Manage Roles] permission.
    ///
    /// # Examples
    ///
    /// See the documentation of [`EditRole`] for details.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] if the current user does not have permission to Manage Roles.
    ///
    /// [Manage Roles]: Permissions::MANAGE_ROLES
    #[inline]
    pub async fn edit(&mut self, http: impl AsRef<Http>, builder: EditRole<'_>) -> Result<()> {
        *self = self.guild_id.edit_role(http.as_ref(), self.id, builder).await?;
        Ok(())
    }

    /// Check that the role has the given permission.
    #[inline]
    #[must_use]
    pub fn has_permission(&self, permission: Permissions) -> bool {
        self.permissions.contains(permission)
    }

    /// Checks whether the role has all of the given permissions.
    ///
    /// The 'precise' argument is used to check if the role's permissions are precisely equivalent
    /// to the given permissions. If you need only check that the role has at least the given
    /// permissions, pass `false`.
    #[inline]
    #[must_use]
    pub fn has_permissions(&self, permissions: Permissions, precise: bool) -> bool {
        if precise {
            self.permissions == permissions
        } else {
            self.permissions.contains(permissions)
        }
    }
}

impl fmt::Display for Role {
    /// Format a mention for the role, pinging its members.
    // This is in the format of: `<@&ROLE_ID>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.mention(), f)
    }
}

impl Eq for Role {}

impl Ord for Role {
    fn cmp(&self, other: &Role) -> Ordering {
        if self.position == other.position {
            self.id.cmp(&other.id)
        } else {
            self.position.cmp(&other.position)
        }
    }
}

impl PartialEq for Role {
    fn eq(&self, other: &Role) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for Role {
    fn partial_cmp(&self, other: &Role) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(feature = "model")]
impl RoleId {
    /// Tries to find the [`Role`] by its Id in the cache.
    #[cfg(feature = "cache")]
    pub fn to_role_cached(self, cache: impl AsRef<Cache>) -> Option<Role> {
        for guild_entry in cache.as_ref().guilds.iter() {
            let guild = guild_entry.value();

            if !guild.roles.contains_key(&self) {
                continue;
            }

            if let Some(role) = guild.roles.get(&self) {
                return Some(role.clone());
            }
        }

        None
    }
}

impl From<Role> for RoleId {
    /// Gets the Id of a role.
    fn from(role: Role) -> RoleId {
        role.id
    }
}

impl<'a> From<&'a Role> for RoleId {
    /// Gets the Id of a role.
    fn from(role: &Role) -> RoleId {
        role.id
    }
}

/// The tags of a [`Role`].
///
/// [Discord docs](https://discord.com/developers/docs/topics/permissions#role-object-role-tags-structure).
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RoleTags {
    /// The Id of the bot the [`Role`] belongs to.
    pub bot_id: Option<UserId>,
    /// The Id of the integration the [`Role`] belongs to.
    pub integration_id: Option<IntegrationId>,
    /// Whether this is the guild's premium subscriber role.
    #[serde(default, skip_serializing_if = "is_false", with = "bool_as_option_unit")]
    pub premium_subscriber: bool,
    /// The id of this role's subscription sku and listing.
    pub subscription_listing_id: Option<SkuId>,
    /// Whether this role is available for purchase.
    #[serde(default, skip_serializing_if = "is_false", with = "bool_as_option_unit")]
    pub available_for_purchase: bool,
    /// Whether this role is a guild's linked role.
    #[serde(default, skip_serializing_if = "is_false", with = "bool_as_option_unit")]
    pub guild_connections: bool,
}

/// A premium subscriber role is reported with the field present and the value `null`.
mod bool_as_option_unit {
    use std::fmt;

    use serde::de::{Error, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        deserializer.deserialize_option(NullValueVisitor)
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S: Serializer>(_: &bool, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }

    struct NullValueVisitor;

    impl<'de> Visitor<'de> for NullValueVisitor {
        type Value = bool;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("null value")
        }

        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            Ok(true)
        }

        /// Called by the `simd_json` crate
        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RoleTags;
    use crate::json::{assert_json, json};

    #[test]
    fn premium_subscriber_role_serde() {
        let value = RoleTags {
            bot_id: None,
            integration_id: None,
            premium_subscriber: true,
            subscription_listing_id: None,
            available_for_purchase: false,
            guild_connections: false,
        };

        assert_json(
            &value,
            json!({"bot_id": null, "integration_id": null, "premium_subscriber": null, "subscription_listing_id": null}),
        );
    }

    #[test]
    fn non_premium_subscriber_role_serde() {
        let value = RoleTags {
            bot_id: None,
            integration_id: None,
            premium_subscriber: false,
            subscription_listing_id: None,
            available_for_purchase: false,
            guild_connections: false,
        };

        assert_json(
            &value,
            json!({"bot_id": null, "integration_id": null, "subscription_listing_id": null}),
        );
    }
}
