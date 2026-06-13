import { fetchInstallScript } from "./_install.js";

export async function onRequest() {
	return fetchInstallScript();
}
