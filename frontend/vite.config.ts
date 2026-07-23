import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Adapter ve compilerOptions svelte.config.js içinde tanımlıdır.
// sveltekit() vite eklentisi argüman almaz; verilen seçenekler yok sayılırdı.
export default defineConfig({
	plugins: [sveltekit()],
	// Tauri geliştirme sunucusu için sabit port (tauri.conf.json devUrl ile uyumlu).
	server: {
		port: 5173,
		strictPort: true
	}
});
