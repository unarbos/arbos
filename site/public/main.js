for (const block of document.querySelectorAll("[data-copy]")) {
	block.addEventListener("click", async () => {
		const text = block.getAttribute("data-copy");
		if (!text) return;
		try {
			await navigator.clipboard.writeText(text);
			block.classList.add("copied");
			window.setTimeout(() => block.classList.remove("copied"), 1200);
		} catch {
			// Clipboard may be blocked; ignore.
		}
	});
}
