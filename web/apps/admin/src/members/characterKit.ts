// The admin Members sheet's routes and wording for the shared roster panel
// (`@kanade/ui` CharacterPanel, dialog, strip and estimate row): an admin
// edits any member's characters, deletes, refreshes and asks members to link.
import type { BossRow, CharacterCreated, CharacterRow, MemberCharacters, MemberRow } from '@kanade/api-types';
import type { CharacterKit } from '@kanade/ui';
import { createdLinkNote } from '../chardata/format';
import { adminEstimateApi, adminLinkApi, chardataRoutes, estimateStep, lookUp, lookupConnected } from '../chardata/routes.svelte';
import { actorName } from '../history/describe';
import { memberLabel } from '../names/directory.svelte';
import { Resource, send } from '../resource.svelte';

export function adminCharacterKit(member: MemberRow, memberName: string, members: MemberRow[], characters: Resource<MemberCharacters>, timeZone: string): CharacterKit {
  const names = (id: string) => memberLabel(members, id);
  const known = (id: string) => members.some((m) => m.id === id);
  return {
    audience: 'admin',
    owner: memberName,
    timeZone,
    routes: chardataRoutes,
    actor: (who) => actorName(who, names, known),
    updatedBy: (row) => actorName(row.updated_by, names, known),
    catalog: new Resource<BossRow[]>('/api/admin/bosses'),
    configHref: '/config?section=character-data',
    onstep: estimateStep,
    api: {
      async refresh() {
        await characters.refresh();
        return characters.data?.characters ?? [];
      },
      patch: (id, body) => send((c) => c.patch<CharacterRow>(`/api/admin/characters/${encodeURIComponent(id)}`, body)),
      async create(body) {
        const created = await send((c) => c.post<CharacterCreated>(`/api/admin/members/${encodeURIComponent(member.id)}/characters`, body));
        if (!created.ok) return created;
        const { link, ...row } = created.value;
        return { ok: true, value: { row, note: createdLinkNote(link, memberName), linkable: false } };
      },
      remove: (id) => send((c) => c.delete<{ message: string }>(`/api/admin/characters/${encodeURIComponent(id)}`)),
      lookupConnected,
      lookUp,
      link: adminLinkApi,
      estimate: (id) => adminEstimateApi({ character_ids: [id] }),
    },
  };
}
