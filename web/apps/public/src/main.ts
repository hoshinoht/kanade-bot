import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/public.scss';
import '@kanade/ui/styles/gate.scss';
import './portal.scss';
import { guardWrites } from '@kanade/client';
import { mount } from 'svelte';
import App from './App.svelte';

// Member writes carry the session's token (the newest any answer sent);
// `GET /api/public/session` issues it.
guardWrites('/api/public/session', (path) => path.startsWith('/api/public/'));

mount(App, { target: document.getElementById('app')! });
