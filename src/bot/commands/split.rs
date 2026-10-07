//! Fit a reply into Discord's message and embed limits: long content is cut
//! at line boundaries into several messages, oversized embeds into several
//! embeds, one embed per message after the first. Lengths are counted in
//! UTF-16 units, never fewer than the characters Discord counts.

use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::EmbedField;

use crate::bot::transport::InteractionReply;

pub const CONTENT_LIMIT: usize = 2000;
const TITLE_LIMIT: usize = 256;
const DESCRIPTION_LIMIT: usize = 4096;
const FIELD_NAME_LIMIT: usize = 256;
const FIELD_VALUE_LIMIT: usize = 1024;
const FOOTER_LIMIT: usize = 2048;
const FIELDS_LIMIT: usize = 25;
const EMBED_TOTAL_LIMIT: usize = 6000;

pub fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest prefix of `text` within `limit` units, cut on a char boundary.
fn prefix(text: &str, limit: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > limit {
            return &text[..at];
        }
    }
    text
}

/// Pieces of at most `limit` units, joined back by `\n` they equal `text`:
/// whole lines are packed together; only a line longer than `limit` is cut.
pub fn split_lines(text: &str, limit: usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut started = false;
    for line in text.split('\n') {
        let mut line = line;
        let joined = if started {
            units(&current) + 1 + units(line)
        } else {
            units(line)
        };
        if joined <= limit {
            if started {
                current.push('\n');
            }
            current.push_str(line);
            started = true;
            continue;
        }
        if started {
            pieces.push(std::mem::take(&mut current));
        }
        while units(line) > limit {
            let head = prefix(line, limit);
            pieces.push(head.to_owned());
            line = &line[head.len()..];
        }
        current.push_str(line);
        started = true;
    }
    if started {
        pieces.push(current);
    }
    pieces
}

fn embed_units(embed: &Embed) -> usize {
    let text = |value: &Option<String>| value.as_deref().map_or(0, units);
    text(&embed.title)
        + text(&embed.description)
        + embed
            .footer
            .as_ref()
            .map_or(0, |footer| units(&footer.text))
        + embed
            .fields
            .iter()
            .map(|field| units(&field.name) + units(&field.value))
            .sum::<usize>()
}

fn continuation(template: &Embed) -> Embed {
    Embed {
        title: None,
        description: None,
        fields: Vec::new(),
        footer: None,
        ..template.clone()
    }
}

/// One embed as several, each within Discord's limits: the title stays on
/// the first, the footer moves to the last, a long description or field
/// value continues at a line boundary.
pub fn fit_embed(embed: &Embed) -> Vec<Embed> {
    let mut first = continuation(embed);
    first.title = embed
        .title
        .as_deref()
        .map(|title| prefix(title, TITLE_LIMIT).to_owned());
    let mut out = vec![first];
    let current = |out: &mut Vec<Embed>| out.len() - 1;
    if let Some(description) = &embed.description {
        for (index, piece) in split_lines(description, DESCRIPTION_LIMIT)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                out.push(continuation(embed));
            }
            let at = current(&mut out);
            out[at].description = Some(piece);
        }
    }
    for field in &embed.fields {
        let name = prefix(&field.name, FIELD_NAME_LIMIT).to_owned();
        for value in split_lines(&field.value, FIELD_VALUE_LIMIT) {
            let piece = EmbedField {
                inline: field.inline,
                name: name.clone(),
                value,
            };
            let at = current(&mut out);
            let room = out[at].fields.len() < FIELDS_LIMIT
                && embed_units(&out[at]) + units(&piece.name) + units(&piece.value)
                    <= EMBED_TOTAL_LIMIT;
            if !room {
                out.push(continuation(embed));
            }
            let at = current(&mut out);
            out[at].fields.push(piece);
        }
    }
    if let Some(footer) = &embed.footer {
        let mut footer = footer.clone();
        footer.text = prefix(&footer.text, FOOTER_LIMIT).to_owned();
        let at = current(&mut out);
        if embed_units(&out[at]) + units(&footer.text) > EMBED_TOTAL_LIMIT {
            out.push(continuation(embed));
        }
        let at = current(&mut out);
        out[at].footer = Some(footer);
    }
    out
}

/// The messages that carry `reply`, in order: content pieces first (the
/// first also carries the first embed), then one embed per message. A
/// Components V2 reply is one message as it stands (its layout is already
/// within budget, see `delivery::cards::redesign::within_budget`).
pub fn split_reply(reply: &InteractionReply) -> Vec<InteractionReply> {
    if !reply.components.is_empty() {
        return vec![reply.clone()];
    }
    let message = |content: String, embeds: Vec<Embed>| InteractionReply {
        content,
        ephemeral: reply.ephemeral,
        embeds,
        components: Vec::new(),
    };
    let mut embeds = reply.embeds.iter().flat_map(fit_embed);
    let mut out: Vec<InteractionReply> = if reply.content.is_empty() {
        Vec::new()
    } else {
        split_lines(&reply.content, CONTENT_LIMIT)
            .into_iter()
            .map(|piece| message(piece, Vec::new()))
            .collect()
    };
    match out.first_mut() {
        Some(first) => first.embeds.extend(embeds.next()),
        None => out.push(message(String::new(), embeds.next().into_iter().collect())),
    }
    out.extend(embeds.map(|embed| message(String::new(), vec![embed])));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use twilight_model::channel::message::embed::EmbedFooter;

    #[test]
    fn lines_pack_and_long_lines_cut_on_char_boundaries() {
        assert_eq!(split_lines("a\nbb\ncc", 4), ["a\nbb", "cc"]);
        assert_eq!(split_lines("", 4), [""]);
        // `é` is one unit, `🧪` two: never split inside either.
        assert_eq!(split_lines("éé🧪é", 3), ["éé", "🧪é"]);
        assert_eq!(
            split_lines("x\nabcdefghij\ny", 4),
            ["x", "abcd", "efgh", "ij\ny"]
        );
        for piece in split_lines(&"🧪".repeat(1500), CONTENT_LIMIT) {
            assert!(units(&piece) <= CONTENT_LIMIT);
        }
    }

    fn embed(fields: usize, value: &str) -> Embed {
        Embed {
            author: None,
            color: Some(1),
            description: Some("d".into()),
            fields: (0..fields)
                .map(|index| EmbedField {
                    inline: false,
                    name: format!("day {index}"),
                    value: value.to_owned(),
                })
                .collect(),
            footer: Some(EmbedFooter {
                icon_url: None,
                proxy_icon_url: None,
                text: "footer".into(),
            }),
            image: None,
            kind: "rich".into(),
            provider: None,
            thumbnail: None,
            timestamp: None,
            title: Some("title".into()),
            url: None,
            video: None,
        }
    }

    #[test]
    fn embeds_split_by_field_count_total_and_value_length() {
        let small = fit_embed(&embed(3, "v"));
        assert_eq!(small, [embed(3, "v")]);

        let line = "x".repeat(300);
        let value = [line.as_str(); 8].join("\n");
        let parts = fit_embed(&embed(30, &value));
        assert!(parts.len() > 1);
        assert_eq!(parts[0].title.as_deref(), Some("title"));
        assert!(parts[1..].iter().all(|part| part.title.is_none()));
        assert!(parts.last().unwrap().footer.is_some());
        let mut lines = 0;
        for part in &parts {
            assert!(part.fields.len() <= FIELDS_LIMIT);
            assert!(embed_units(part) <= EMBED_TOTAL_LIMIT);
            for field in &part.fields {
                assert!(units(&field.value) <= FIELD_VALUE_LIMIT);
                lines += field.value.split('\n').count();
            }
        }
        assert_eq!(lines, 30 * 8, "every line kept once");
    }

    #[test]
    fn replies_keep_order_and_visibility() {
        let text = vec!["y".repeat(900); 5].join("\n");
        let reply = InteractionReply::ephemeral(text.clone()).with_embed(embed(1, "v"));
        let parts = split_reply(&reply);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.ephemeral));
        assert_eq!(parts[0].embeds.len(), 1);
        let joined: Vec<&str> = parts.iter().map(|part| part.content.as_str()).collect();
        assert_eq!(joined.join("\n"), text);
        assert_eq!(
            split_reply(&InteractionReply::public("hi")),
            [InteractionReply::public("hi")]
        );
    }
}
