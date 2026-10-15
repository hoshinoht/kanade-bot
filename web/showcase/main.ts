// The core primitives showcase (dev-only): `bun run showcase` in web/ builds
// it into apps/admin/dist/__showcase/, which the e2e server serves at /__showcase/
// with the admin origin's production CSP. Core partials only, no screen
// partial: what renders here is what every screen builds on.
import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import './core.scss';
import './showcase.css';
import { mount } from 'svelte';
import Showcase from './Showcase.svelte';

mount(Showcase, { target: document.getElementById('app')! });
