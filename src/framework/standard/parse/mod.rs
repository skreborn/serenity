use super::{Command, *};
use crate::client::Context;
use crate::model::prelude::*;

pub mod map;

use std::borrow::Cow;
#[cfg(feature = "cache")]
use std::collections::HashMap;

use futures::future::{BoxFuture, FutureExt};
use map::{CommandMap, GroupMap, ParseMap};
use uwl::Stream;

// FIXME: Add the `http` parameter to `Guild::user_permissions_in`.
//
// Trying to shove the parameter to the original method results in several errors and interface
// changes to methods using `Guild::user_permissions_in` that are not worthwhile to resolve. As a
// compromise, the method has been copied with the parameter added in to the place where the
// *problem* occurs.
//
// When a bot's command is invoked in a large guild (e.g., 250k+ members), the method fails to
// retrieve the member data of the author that invoked the command, and instead defaults to
// `@everyone`'s permissions. This is because Discord does not send data of all members past 250,
// resulting in the problem to meet permissions of a command even if the author does possess them.
// To avoid defaulting to permissions of everyone, we fetch the member from HTTP if it is missing
// in the guild's members list.
#[cfg(feature = "cache")]
fn permissions_in(
    ctx: &Context,
    guild_id: GuildId,
    channel_id: ChannelId,
    member: &Member,
    roles: &HashMap<RoleId, Role>,
) -> Permissions {
    let guild = ctx.cache.guild(guild_id);
    if guild.as_ref().map(|guild| member.user.id == guild.owner_id) == Some(true) {
        return Permissions::all();
    }

    let Some(everyone) = roles.get(&RoleId(guild_id.0)) else {
        tracing::error!("@everyone role is missing in guild {}", guild_id);

        return Permissions::empty();
    };

    let mut permissions = everyone.permissions;

    for &role in &member.roles {
        if let Some(role) = roles.get(&role) {
            permissions |= role.permissions;
        } else {
            tracing::warn!("{} on {} has non-existent role {:?}", member.user.id, guild_id, role);
        }
    }

    if permissions.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }

    if let Some(channel) = guild.and_then(|guild| guild.channels.get(&channel_id).cloned()) {
        let mut data = Vec::with_capacity(member.roles.len());

        for overwrite in &channel.permission_overwrites {
            if let PermissionOverwriteType::Role(role) = overwrite.kind {
                if role.0 != guild_id.0 && !member.roles.contains(&role) {
                    continue;
                }

                if let Some(role) = roles.get(&role) {
                    data.push((role.position, overwrite.deny, overwrite.allow));
                }
            }
        }

        data.sort_by(|a, b| a.0.cmp(&b.0));

        for overwrite in data {
            permissions = (permissions & !overwrite.1) | overwrite.2;
        }

        for overwrite in &channel.permission_overwrites {
            if PermissionOverwriteType::Member(member.user.id) != overwrite.kind {
                continue;
            }

            permissions = (permissions & !overwrite.deny) | overwrite.allow;
        }
    } else {
        tracing::warn!("Guild {} does not contain channel {}", guild_id, channel_id);
    }

    if channel_id.0 == guild_id.0 {
        permissions |= Permissions::VIEW_CHANNEL;
    }

    permissions
}

#[inline]
fn to_lowercase<'a>(config: &Configuration, s: &'a str) -> Cow<'a, str> {
    if config.case_insensitive {
        Cow::Owned(s.to_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// Parse a mention in the message that is of either the direct (`<@id>`) or nickname (`<@!id>`)
/// syntax, and compare the encoded `id` with the id from [`Configuration::on_mention`] for a
/// match. Returns `Some(<id>)` on success, [`None`] otherwise.
pub fn mention<'a>(stream: &mut Stream<'a>, config: &Configuration) -> Option<&'a str> {
    let on_mention = config.on_mention.as_deref()?;

    let start = stream.offset();

    if !stream.eat("<@") {
        return None;
    }

    // Optional.
    stream.eat("!");

    let id = stream.take_while(|b| b.is_ascii_digit());

    if !stream.eat(">") {
        // Backtrack to where we were.
        stream.set(start);

        return None;
    }

    if id == on_mention {
        Some(id)
    } else {
        stream.set(start);

        None
    }
}

async fn find_prefix<'a>(
    ctx: &Context,
    msg: &Message,
    config: &Configuration,
    stream: &Stream<'a>,
) -> Option<Cow<'a, str>> {
    let try_match = |prefix: &str| {
        let peeked = stream.peek_for_char(prefix.chars().count());
        let peeked = to_lowercase(config, peeked);
        (prefix == peeked).then_some(peeked)
    };

    for f in &config.dynamic_prefixes {
        if let Some(p) = f(ctx, msg).await {
            let p = to_lowercase(config, &p);
            if let Some(p) = try_match(&p) {
                return Some(p);
            }
        }
    }

    config.prefixes.iter().find_map(|p| try_match(p))
}

/// Parse a prefix in the message.
///
/// The "prefix" may be one of the following:
/// - A mention (`<@id>`/`<@!id>`)
/// - A dynamically constructed prefix ([`Configuration::dynamic_prefix`])
/// - A static prefix ([`Configuration::prefix`])
/// - Nothing
///
/// In all cases, whitespace after the prefix is cleared.
#[allow(clippy::needless_lifetimes)] // Clippy and the compiler disagree
pub async fn prefix<'a>(
    ctx: &Context,
    msg: &Message,
    stream: &mut Stream<'a>,
    config: &Configuration,
) -> Option<Cow<'a, str>> {
    if let Some(id) = mention(stream, config) {
        stream.take_while_char(char::is_whitespace);

        return Some(Cow::Borrowed(id));
    }

    let prefix = find_prefix(ctx, msg, config, stream).await;

    if let Some(prefix) = &prefix {
        stream.increment(prefix.len());
    }

    if config.with_whitespace.prefixes {
        stream.take_while_char(char::is_whitespace);
    }

    prefix
}

/// Checked per valid group or command in the message.
async fn check_discrepancy(
    #[allow(unused_variables)] ctx: &Context,
    msg: &Message,
    config: &Configuration,
    options: &impl CommonOptions,
) -> Result<(), DispatchError> {
    if options.owners_only() && !config.owners.contains(&msg.author.id) {
        return Err(DispatchError::OnlyForOwners);
    }

    if options.only_in() == OnlyIn::Dm && !msg.is_private() {
        return Err(DispatchError::OnlyForDM);
    }

    if (!config.allow_dm || options.only_in() == OnlyIn::Guild) && msg.is_private() {
        return Err(DispatchError::OnlyForGuilds);
    }

    #[cfg(feature = "cache")]
    {
        if let Some(guild_id) = msg.guild_id {
            let roles = match ctx.cache.guild(guild_id) {
                Some(guild) => guild.roles.clone(),
                None => return Ok(()),
            };

            let Ok(member) = guild_id.member(ctx, msg.author.id).await else {return Ok(())};
            let perms = permissions_in(ctx, guild_id, msg.channel_id, &member, &roles);

            if !(perms.contains(*options.required_permissions())
                || options.owner_privilege() && config.owners.contains(&msg.author.id))
            {
                return Err(DispatchError::LackingPermissions(*options.required_permissions()));
            }

            if !perms.administrator() && !has_correct_roles(options, &roles, &member) {
                return Err(DispatchError::LackingRole);
            }
        }
    }

    Ok(())
}

fn try_parse<M: ParseMap>(
    stream: &mut Stream<'_>,
    map: &M,
    by_space: bool,
    f: impl Fn(&str) -> String,
) -> (String, Option<M::Storage>) {
    if by_space {
        let n = f(stream.peek_until_char(char::is_whitespace));

        let o = map.get(&n);

        (n, o)
    } else {
        let mut n = f(stream.peek_for_char(map.max_length()));
        let mut o = None;

        for _ in 0..(map.max_length() - map.min_length()) {
            o = map.get(&n);

            if o.is_some() {
                break;
            }

            n.pop();
        }

        (n, o)
    }
}

fn parse_cmd<'a>(
    stream: &'a mut Stream<'_>,
    ctx: &'a Context,
    msg: &'a Message,
    config: &'a Configuration,
    map: &'a CommandMap,
) -> BoxFuture<'a, Result<&'static Command, ParseError>> {
    async move {
        let (n, r) =
            try_parse(stream, map, config.by_space, |s| to_lowercase(config, s).into_owned());

        if config.disabled_commands.contains(&n) {
            return Err(ParseError::Dispatch {
                error: DispatchError::CommandDisabled,
                command_name: n,
            });
        }

        if let Some((cmd, map)) = r {
            stream.increment(n.len());

            if config.with_whitespace.commands {
                stream.take_while_char(char::is_whitespace);
            }

            check_discrepancy(ctx, msg, config, &cmd.options).await.map_err(|e| {
                ParseError::Dispatch {
                    error: e,
                    command_name: n,
                }
            })?;

            if map.is_empty() {
                return Ok(cmd);
            }

            return match parse_cmd(stream, ctx, msg, config, &map).await {
                Err(ParseError::UnrecognisedCommand(Some(_))) => Ok(cmd),
                res => res,
            };
        }

        Err(ParseError::UnrecognisedCommand(Some(n.to_string())))
    }
    .boxed()
}

fn parse_group<'a>(
    stream: &'a mut Stream<'_>,
    ctx: &'a Context,
    msg: &'a Message,
    config: &'a Configuration,
    map: &'a GroupMap,
) -> BoxFuture<'a, Result<(&'static CommandGroup, Arc<CommandMap>), ParseError>> {
    async move {
        let (n, o) = try_parse(stream, map, config.by_space, ToString::to_string);

        if let Some((group, map, commands)) = o {
            stream.increment(n.len());

            if config.with_whitespace.groups {
                stream.take_while_char(char::is_whitespace);
            }

            check_discrepancy(ctx, msg, config, &group.options).await.map_err(|e| {
                ParseError::Dispatch {
                    error: e,
                    command_name: n,
                }
            })?;

            if map.is_empty() {
                return Ok((group, commands));
            }

            return match parse_group(stream, ctx, msg, config, &map).await {
                Err(ParseError::UnrecognisedCommand(None)) => Ok((group, commands)),
                res => res,
            };
        }

        Err(ParseError::UnrecognisedCommand(None))
    }
    .boxed()
}

#[inline]
async fn handle_command<'a>(
    stream: &'a mut Stream<'_>,
    ctx: &'a Context,
    msg: &'a Message,
    config: &'a Configuration,
    map: &'a CommandMap,
    group: &'static CommandGroup,
) -> Result<Invoke, ParseError> {
    match parse_cmd(stream, ctx, msg, config, map).await {
        Ok(command) => Ok(Invoke::Command {
            group,
            command,
        }),
        Err(err) => match group.options.default_command {
            Some(command) => {
                check_discrepancy(ctx, msg, config, &command.options).await.map_err(|e| {
                    ParseError::Dispatch {
                        error: e,
                        command_name: command.options.names[0].to_string(),
                    }
                })?;

                Ok(Invoke::Command {
                    group,
                    command,
                })
            },
            None => Err(err),
        },
    }
}

#[inline]
async fn handle_group<'a>(
    stream: &mut Stream<'_>,
    ctx: &'a Context,
    msg: &'a Message,
    config: &'a Configuration,
    map: &'a GroupMap,
) -> Result<Invoke, ParseError> {
    match parse_group(stream, ctx, msg, config, map).await {
        Ok((group, map)) => handle_command(stream, ctx, msg, config, &map, group).await,
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
pub enum ParseError {
    UnrecognisedCommand(Option<String>),
    Dispatch { error: DispatchError, command_name: String },
}

fn is_unrecognised<T>(res: &Result<T, ParseError>) -> bool {
    matches!(res, Err(ParseError::UnrecognisedCommand(_)))
}

/// Parse a command from the message.
///
/// The "command" may be:
/// 1. A *help command* that provides a friendly browsing interface of all groups and commands,
///    explaining what each of them are, how they are laid out and how to invoke them. There can
///    only one help command registered, but might have many names defined for invocation of itself.
///
/// 2. A command defined under another command or a group, which may also belong to another group
///    and so on. To invoke this command, all names and prefixes of its parent commands and groups
///    must be specified before it.
pub async fn command(
    ctx: &Context,
    msg: &Message,
    stream: &mut Stream<'_>,
    groups: &[(&'static CommandGroup, Map)],
    config: &Configuration,
    help_was_set: Option<&[&'static str]>,
) -> Result<Invoke, ParseError> {
    // Precedence is taken over commands named as one of the help names.
    if let Some(names) = help_was_set {
        for name in names {
            let n = to_lowercase(config, stream.peek_for_char(name.chars().count()));

            if name == &n {
                stream.increment(n.len());

                stream.take_while_char(char::is_whitespace);

                return Ok(Invoke::Help(name));
            }
        }
    }

    let mut last = Err::<Invoke, _>(ParseError::UnrecognisedCommand(None));
    let mut is_prefixless = false;

    for (group, map) in groups {
        match map {
            // Includes [group] itself.
            Map::WithPrefixes(map) => {
                let res = handle_group(stream, ctx, msg, config, map).await;

                if !is_unrecognised(&res) {
                    return res;
                }

                if !is_prefixless {
                    last = res;
                }
            },
            Map::Prefixless(subgroups, commands) => {
                fn command_name_if_recognised(res: &Result<Invoke, ParseError>) -> Option<&str> {
                    match res {
                        Ok(Invoke::Command {
                            command, ..
                        }) => Some(command.options.names[0]),
                        Ok(Invoke::Help(name)) => Some(name), // unreachable; fallback just in case
                        Err(ParseError::UnrecognisedCommand(_)) => None,
                        Err(ParseError::Dispatch {
                            command_name, ..
                        }) => Some(command_name),
                    }
                }

                is_prefixless = true;

                let res = handle_group(stream, ctx, msg, config, subgroups).await;

                if let Some(command_name) = command_name_if_recognised(&res) {
                    check_discrepancy(ctx, msg, config, &group.options).await.map_err(|e| {
                        ParseError::Dispatch {
                            error: e,
                            command_name: command_name.to_owned(),
                        }
                    })?;
                    return res;
                }

                let res = handle_command(stream, ctx, msg, config, commands, group).await;

                if let Some(command_name) = command_name_if_recognised(&res) {
                    check_discrepancy(ctx, msg, config, &group.options).await.map_err(|e| {
                        ParseError::Dispatch {
                            error: e,
                            command_name: command_name.to_owned(),
                        }
                    })?;
                    return res;
                }

                last = res;
            },
        }
    }

    last
}

#[derive(Debug)]
pub enum Invoke {
    Command { group: &'static CommandGroup, command: &'static Command },
    Help(&'static str),
}
