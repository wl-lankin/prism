import '@fontsource-variable/inter';
import '@fontsource-variable/space-grotesk';
import './lib/theme.css';
import { mount } from 'svelte';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });
