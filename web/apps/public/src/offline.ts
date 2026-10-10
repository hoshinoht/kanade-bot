import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/public.scss';
import { mount } from 'svelte';
import Offline from './Offline.svelte';

mount(Offline, { target: document.getElementById('app')! });
