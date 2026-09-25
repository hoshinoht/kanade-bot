import '@kanade/tokens/index.scss';
import '@kanade/tokens/fonts.css';
import '@kanade/ui/admin.scss';
import { mount } from 'svelte';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });
