import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/admin.scss';
import { guardWrites } from '@kanade/client';
import { mount } from 'svelte';
import App from './App.svelte';
import { initExperiments } from '@kanade/ui';

// admin-api API-5: admin writes carry the session's token; sign-in needs only the same-origin markers.
guardWrites('/api/admin/session', (path) => path.startsWith('/api/admin/') && !/^\/api\/admin\/auth\/(?:token|tailscale)(?:[/?]|$)/.test(path));

initExperiments();
mount(App, { target: document.getElementById('app')! });
