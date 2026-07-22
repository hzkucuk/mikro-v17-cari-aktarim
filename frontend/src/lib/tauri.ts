import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';

/** Tauri 2 API'sinin UI'dan tek, tiplenebilir erişim noktası. */
export const invoke = <T>(command: string, args?: Record<string, unknown>) =>
	tauriInvoke<T>(command, args);

export const listen = tauriListen;
