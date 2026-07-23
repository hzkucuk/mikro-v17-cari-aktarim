import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	compilerOptions: {
		// Svelte 6'da kaldırılabilir; şimdilik runes modunu zorluyoruz.
		runes: true
	},
	kit: {
		// Tauri webview'i tek sayfalık statik bir SPA çalıştırır.
		// fallback ile tüm rotalar index.html'e düşer; SSR yoktur (+layout.ts).
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: 'index.html',
			precompress: false,
			strict: true
		})
	}
};

export default config;
