/** Portrait URLs on the admin origin (session cookie; ETag-revalidated). */
export const memberAvatar = (id: string) => `/api/admin/members/${encodeURIComponent(id)}/avatar`;
export const ME_AVATAR = '/api/admin/me/avatar';
