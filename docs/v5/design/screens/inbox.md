# Inbox

**Purpose:** decide what Kanade proposes (Extractor) and what members ask
(Self-service); look back at closed proposals (Past). Budget: the selected
item's detail > the list > tabs/counts. `[guide]`

**Boards:** `VarRail2` (Extractor, long thread), `B_InboxSelf`, `B_Empty`,
`B_PhoneInbox`. Pairs: inbox-extractor, inbox-self, inbox-empty,
phone-inbox (all approved 2026-10-02). `Inbox.dc.html` is superseded by
`VarRail2`. `[DR 2026-10-02]`

## As built

Code: `apps/admin/src/inbox/` (`InboxPage`, `InboxList`, `InboxDetail`,
`InboxEmpty`, `OutcomeChip`, `PastList`, `PastDetail`, `expiry.ts`,
`edit.ts`, `flags.ts`); `components/DecisionCard.svelte`, `ThreadPanel.svelte`;
`_inbox.scss`.

- One window, title-bar tabs **Extractor n / Self-service n / Past**,
  deep links `?tab=&item=`; list pane 300 px (`_inbox.scss:18`) beside the
  detail; below 900 px list and detail take turns.
- Detail: header (portrait, heading with boss tags that wrap whole at every
  width, summary) → thread panel (full thread, per-message "used" marker,
  Used n / All toggle `[DR 2026-10-02]`; on a narrow thread the head takes
  two rows and the foot wraps rather than cutting a fact `[DR 2026-10-05]`)
  + decision card (300 px,
  `--select`, radius 28): mono overline "Would change", old value struck,
  new value, **consequence line** before Approve when the API supplies it
  `[DR 2026-10-03]`, key **Approve**, "Edit, then approve" field + Move,
  Reject… at the foot; proposal expiry as a flat bar, warn near the end
  `[DR 2026-10-04]`.
- Self-service: state chips (conflict, expired, requester not allowed, already
  in effect), the member's words, Would change card, "Changed since the member
  asked" box; a blocked Approve stays visible, `aria-disabled` and focusable
  with a one-line reason, and Reject… becomes the risk key.
  `[old §Inbox: Self-service]`
- Past tab: read-only closed proposals, newest first, with outcome, decider,
  time, reason, source links and a History link when approved.
  `[DR 2026-10-04]`
- Empty: `StateNote` "Nothing waiting" + recently decided (text-only rows,
  `[DR 2026-10-04]`).

## Phone

`inbox--compact`: an open item drops the page line, tabs and window chrome,
puts "‹ Inbox" in the top bar (`chrome.back()`), shows a compact header
(its facts wrap to a second line, never cut, so "See the card" shows in
full `[DR 2026-10-05]`),
thread toggle and the decision as a bottom action bar (pencil toggles the
edit field, Reject…, Approve). The consequence line sits inside the change
card on phones and Self-service. `[web/AGENTS]`, `[DR 2026-10-03]`

## Known gaps / Planned

- Accepted residual fidelity findings: +1 flexWrap on heading tags; five
  phone-inbox findings with the consequence line. `[DR 2026-10-03]`
- **Planned:** member avatars in thread rows (server-cached gateway avatars,
  monogram fallback) after the fidelity gate. `[DR 2026-10-02]`
- **Planned:** arrival motion for new items (live updates).
