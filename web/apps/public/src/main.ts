import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/public.scss';
import '@kanade/ui/styles/gate.scss';
import '@kanade/ui/styles/account.scss';
// The Week window, its progress bars, the run pane's art and the Move picker's look (admin Week,
// without its admin-only parts: week/admin-week.scss).
import '@kanade/ui/styles/progress.scss';
import './week/admin-week.scss';
import '@kanade/ui/styles/move-picker.scss';
import '@kanade/ui/styles/row-expansion.scss';
import './portal.scss';
import './week/week.scss';
import { guardWrites } from '@kanade/client';
import { mount } from 'svelte';
import App from './App.svelte';

// Member writes carry the session's token (the newest any answer sent);
// `GET /api/public/session` issues it.
guardWrites('/api/public/session', (path) => path.startsWith('/api/public/'));

mount(App, { target: document.getElementById('app')! });
