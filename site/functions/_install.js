/** @typedef {import('@cloudflare/workers-types').Request} Request */
/** @typedef {import('@cloudflare/workers-types').Response} Response */

export const INSTALL_SCRIPT_URL =
	"https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh";

export const LAUNCHER_SCRIPT_URL =
	"https://raw.githubusercontent.com/unarbos/arbos/main/scripts/arbos";

/** @returns {Promise<Response>} */
export async function fetchInstallScript() {
	const resp = await fetch(INSTALL_SCRIPT_URL, {
		cf: { cacheTtl: 300, cacheEverything: true },
	});
	if (!resp.ok) {
		return new Response("install script unavailable", { status: 502 });
	}
	return new Response(resp.body, {
		headers: {
			"Content-Type": "text/plain; charset=utf-8",
			"Cache-Control": "public, max-age=300",
		},
	});
}

/** @returns {Promise<Response>} */
export async function fetchLauncherScript() {
	const resp = await fetch(LAUNCHER_SCRIPT_URL, {
		cf: { cacheTtl: 300, cacheEverything: true },
	});
	if (!resp.ok) {
		return new Response("launcher script unavailable", { status: 502 });
	}
	return new Response(resp.body, {
		headers: {
			"Content-Type": "text/plain; charset=utf-8",
			"Cache-Control": "public, max-age=300",
		},
	});
}

/** @param {Request} request */
export function wantsInstallScript(request) {
	const accept = request.headers.get("Accept") ?? "";
	const ua = request.headers.get("User-Agent") ?? "";
	return (
		ua.includes("curl") ||
		ua.includes("wget") ||
		!accept.includes("text/html")
	);
}
