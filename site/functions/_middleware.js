import { fetchInstallScript, fetchLauncherScript, wantsInstallScript } from "./_install.js";

/** @param {import('@cloudflare/workers-types').EventContext} context */
export async function onRequest(context) {
	const url = new URL(context.request.url);
	if (url.pathname === "/arbos" && wantsInstallScript(context.request)) {
		return fetchLauncherScript();
	}
	if (url.pathname === "/" && wantsInstallScript(context.request)) {
		return fetchInstallScript();
	}
	return context.next();
}
