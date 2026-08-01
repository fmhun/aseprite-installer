import { copyFile, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repositoryRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const clientDirectory = join(repositoryRoot, "dist-site");
const serverDirectory = join(repositoryRoot, "dist-site-ssr");
const clientHtmlPath = join(clientDirectory, "index.html");
const serverEntryPath = join(serverDirectory, "entry-server.js");
const marker = '<div id="root" data-prerendered="false"><!--app-html--></div>';

try {
  const [{ render }, template] = await Promise.all([
    import(pathToFileURL(serverEntryPath).href),
    readFile(clientHtmlPath, "utf8"),
  ]);

  if (!template.includes(marker)) {
    throw new Error("The site template is missing its static-render marker.");
  }

  const appHtml = render();
  if (!appHtml.includes("<main") || !appHtml.includes("<h1")) {
    throw new Error("The server render is missing the landing page content.");
  }

  const prerenderedHtml = template.replace(
    marker,
    `<div id="root" data-prerendered="true">${appHtml}</div>`,
  );

  await Promise.all([
    writeFile(clientHtmlPath, prerenderedHtml),
    copyFile(
      join(repositoryRoot, "assets/icons/aseprite-installer.svg"),
      join(clientDirectory, "favicon.svg"),
    ),
  ]);
} finally {
  await rm(serverDirectory, { recursive: true, force: true });
}
