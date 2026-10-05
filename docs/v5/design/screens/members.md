# Members

**Purpose:** the roster: runs this week, @mention preference, reply style,
aliases; edit one member.

**Board:** `B_Members` (pair: members, approved 2026-10-02).

## As built

Code: `apps/admin/src/members/` (`MembersPage`, `MemberSheet`, `runs.ts`);
`_members.scss`.

- One window titled Roster: search, **Sort** dropdown (default "runs": runs this
  week descending, A–Z fallback; app-only) `[DR 2026-10-02]`.
- List: grid `Member | This wk | @mentions | Reply style`
  (`minmax(0,1fr) 70px 96px 100px`, `_members.scss:49`); the name cell keeps
  the name whole and drops the alias chip below `[DR 2026-10-03]`.
- Detail: side pane `min(380px, 38vw)` from 840 px (`MemberSheet.svelte`);
  below 840 px a modal. Header (avatar, name, Discord handle) → key/value grid
  (`128px 1fr`) → @mentions `.seg` Essential / All / Off with hint → Reply
  style dropdown → Chat aliases → This week (grouped rows).

## Phone

Modal sheet; alias chips fit the dialog (`e2e/phone-fit.spec.ts`).

## Known gaps / Planned

- **Planned (2026-10-05):** the pane/single-pane switch moves from 840 px to
  the shared 900 px breakpoint (`../shell-and-components.md`).
- **Planned:** alias × removal (`DELETE …/aliases/{alias}` + mock + types);
  chips have no × until then. `[DR 2026-10-02]`
- **Planned:** member avatars (`/members/{id}/avatar`, monogram fallback).
  `[DR 2026-10-02]`
