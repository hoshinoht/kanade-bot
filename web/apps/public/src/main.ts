import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/public.scss';
import '@kanade/ui/styles/screens/gate.scss';
import '@kanade/ui/styles/screens/account.scss';
// The Week window, its progress bars, the run pane's art and the Move picker's look (admin Week,
// without its admin-only parts: week/admin-week.scss).
import '@kanade/ui/styles/core/progress.scss';
import './week/admin-week.scss';
import '@kanade/ui/styles/screens/move-picker.scss';
import '@kanade/ui/styles/core/row-expansion.scss';
import '@kanade/ui/styles/screens/boss-stack.scss';
import './portal.scss';
import './week/week.scss';
// Weekly timings' Hand to… and Confirm it's you (the shared Modal's look).
import '@kanade/ui/styles/core/modal.scss';
import './timings/timings.scss';
// The admin's motion that the member screens share: arrival marks and number
// ticks (week reads from elsewhere) and the loading standard's morphing shape
// (LoadingState renders it; Experiment A, on by default).
import '@kanade/ui/styles/core/arrival.scss';
import '@kanade/ui/styles/core/loading.scss';
import { guardWrites } from '@kanade/client';
import { mount } from 'svelte';
import App from './App.svelte';

// Member writes carry the session's token (the newest any answer sent);
// `GET /api/public/session` issues it.
guardWrites('/api/public/session', (path) => path.startsWith('/api/public/'));

mount(App, { target: document.getElementById('app')! });
