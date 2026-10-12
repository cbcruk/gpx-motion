import './style.css'
import { startDemo } from './demo/demo'

const root = document.querySelector<HTMLElement>('#app')
if (!root) throw new Error('index.html has no #app element')
const stop = startDemo(root)
// In development, `stopGpxMotion()` in the console tears the demo down.
if (import.meta.env.DEV) Object.assign(window, { stopGpxMotion: stop })
