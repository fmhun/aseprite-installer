import { access, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outputDirectory = join(repositoryRoot, "dist-site");
const canonicalUrl = "https://fmhun.github.io/aseprite-installer/";
const basePath = "/aseprite-installer/";
const packageManifest = JSON.parse(await readFile(join(repositoryRoot, "package.json"), "utf8"));
const releaseVersion = packageManifest.version;
const releaseTag = `v${releaseVersion}`;
const releaseUrl = `https://github.com/fmhun/aseprite-installer/releases/tag/${releaseTag}`;
const releasePublishedDate = "2026-08-01";
const latestAssetBaseUrl = "https://github.com/fmhun/aseprite-installer/releases/latest/download/";
const releaseAssetNames = [
  "Aseprite-Installer-macOS-arm64.dmg",
  "Aseprite-Installer-macOS-arm64.app.zip",
  "Aseprite-Installer-macOS-x64.dmg",
  "Aseprite-Installer-macOS-x64.app.zip",
  "Aseprite-Installer-Windows-x64-setup.exe",
  "Aseprite-Installer-Windows-x64.msi",
  "Aseprite-Installer-Linux-x86_64.AppImage",
  "Aseprite-Installer-Linux-x86_64.deb",
  "Aseprite-Installer-Linux-x86_64.rpm",
];
const expectedDownloadUrls = releaseAssetNames.map(
  (assetName) => `https://github.com/fmhun/aseprite-installer/releases/download/${releaseTag}/${assetName}`,
);
const expectedLatestAssetUrls = [...releaseAssetNames, "SHA256SUMS"].map(
  (assetName) => `${latestAssetBaseUrl}${assetName}`,
);
const expectedOperatingSystems = [
  "macOS 15.2 or later",
  "Windows 11",
  "Linux x86_64 compatible with the Ubuntu 22.04 or Debian 12 runtime baseline",
];
const expectedProcessorRequirements =
  "macOS: Apple Silicon (arm64) or Intel (x86_64); Windows 11: x86_64; Linux: x86_64";

function invariant(condition, message) {
  if (!condition) throw new Error(`SEO validation failed: ${message}`);
}

function occurrences(value, pattern) {
  return [...value.matchAll(pattern)].length;
}

function decodeHtmlMetadata(value) {
  return value.replaceAll("&amp;", "&");
}

const [html, robots, sitemap, htmlStats, sourceStyles, readme] = await Promise.all([
  readFile(join(outputDirectory, "index.html"), "utf8"),
  readFile(join(outputDirectory, "robots.txt"), "utf8"),
  readFile(join(outputDirectory, "sitemap.xml"), "utf8"),
  stat(join(outputDirectory, "index.html")),
  readFile(join(repositoryRoot, "site/src/styles.css"), "utf8"),
  readFile(join(repositoryRoot, "README.md"), "utf8"),
]);

invariant(htmlStats.size < 150_000, "the initial HTML exceeds the 150 KB crawl budget");
invariant(html.includes('data-prerendered="true"'), "the React root is not pre-rendered");
invariant(!html.includes("<!--app-html-->"), "the pre-render placeholder remains in the build");
invariant(/<main\s+[^>]*id="main"[^>]*>/i.test(html), "the initial HTML does not contain the main landmark");
invariant(occurrences(html, /<h1(?:\s|>)/g) === 1, "the page must expose exactly one H1");
invariant(html.includes("Install <em>Aseprite</em>"), "the H1 is missing from the initial HTML");
invariant(html.includes("Choose your platform. Build locally."), "the multi-platform install section is missing");
invariant(html.includes("macOS 15.2+"), "the macOS support baseline is missing");
invariant(html.includes("Windows 11"), "the Windows support baseline is missing");
invariant(html.includes("Linux x86_64"), "the Linux architecture is missing");
invariant(html.includes("open the DMG"), "the macOS installation path is missing");
invariant(html.includes("run the current-user installer"), "the Windows installation path is missing");
invariant(html.includes("run the AppImage"), "the Linux installation path is missing");
invariant(html.includes("Which installer should I choose?"), "the platform package FAQ is missing");
invariant(html.includes("Does the installer distribute Aseprite?"), "the FAQ is missing from the initial HTML");
invariant(html.includes("Aseprite remains subject to its own EULA"), "the Aseprite license distinction is missing");
invariant(html.includes('href="https://github.com/fmhun/aseprite-installer/releases/latest"'), "the latest release link is missing");
invariant(!html.includes("Aseprite-Installer-macOS-Universal"), "the page still references the retired universal DMG");
invariant(!html.includes("18 GB"), "the obsolete disk-space requirement remains");
invariant(
  readme.includes("[Download for macOS, Windows, or Linux](https://github.com/fmhun/aseprite-installer/releases/latest)"),
  "the README does not point every platform to the latest release",
);
invariant(!readme.includes("Aseprite-Installer-macOS-Universal"), "the README still references the retired universal DMG");

const title = decodeHtmlMetadata(html.match(/<title>([^<]+)<\/title>/)?.[1] ?? "");
const description = html.match(/<meta\s+name="description"\s+content="([^"]+)"/s)?.[1] ?? "";
const openGraphTitle = decodeHtmlMetadata(html.match(/<meta\s+property="og:title"\s+content="([^"]+)"/s)?.[1] ?? "");
const openGraphDescription = html.match(/<meta\s+property="og:description"\s+content="([^"]+)"/s)?.[1] ?? "";
const twitterTitle = decodeHtmlMetadata(html.match(/<meta\s+name="twitter:title"\s+content="([^"]+)"/s)?.[1] ?? "");
const twitterDescription = html.match(/<meta\s+name="twitter:description"\s+content="([^"]+)"/s)?.[1] ?? "";
invariant(title.length >= 30 && title.length <= 60, "the title should be 30–60 characters");
invariant(description.length >= 120 && description.length <= 160, "the meta description should be 120–160 characters");
invariant(title === "Aseprite Installer for macOS, Windows & Linux", "the title does not describe every supported platform");
invariant(description.includes("free, open-source installer"), "the meta description does not distinguish the free installer");
invariant(description.includes("macOS, Windows 11, or Linux"), "the meta description omits a supported platform");
invariant(openGraphTitle === title, "the Open Graph title must match the document title");
invariant(openGraphDescription === description, "the Open Graph description must match the meta description");
invariant(twitterTitle === title, "the X card title must match the document title");
invariant(twitterDescription === description, "the X card description must match the meta description");
invariant(occurrences(html, /rel="canonical"/g) === 1, "the page must have exactly one canonical URL");
invariant(html.includes(`rel="canonical" href="${canonicalUrl}"`), "the canonical URL is incorrect");
invariant(html.includes('name="robots" content="index, follow,'), "the index/follow robots directive is missing");
invariant(!/noindex|nofollow/i.test(html), "the page contains a blocking robots directive");
invariant(!/name="keywords"/i.test(html), "meta keywords must not be used");
invariant(html.includes('property="og:locale" content="en_US"'), "the Open Graph locale is missing");
invariant(html.includes('property="og:image:type" content="image/png"'), "the Open Graph image type is missing");
invariant(html.includes('property="og:image:width" content="1200"'), "the Open Graph image width is missing");
invariant(html.includes('property="og:image:height" content="630"'), "the Open Graph image height is missing");
invariant(html.includes('name="twitter:card" content="summary_large_image"'), "the X card metadata is incomplete");
invariant(html.includes(`href="${basePath}favicon.svg"`), "the stable favicon URL is missing");
invariant(html.includes('name="color-scheme" content="dark"'), "the page must advertise a dark-only color scheme");
invariant(!html.includes('name="color-scheme" content="dark light"'), "the page still advertises a light color scheme");
invariant(sourceStyles.includes("color-scheme: only dark;"), "the stylesheet must force the dark color scheme");
invariant(!sourceStyles.includes("@media (prefers-color-scheme: light)"), "the stylesheet still contains a system light-theme override");

const structuredDataSource = html.match(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/)?.[1];
invariant(Boolean(structuredDataSource), "JSON-LD structured data is missing");
const structuredData = JSON.parse(structuredDataSource);
const graph = structuredData["@graph"];
invariant(Array.isArray(graph), "JSON-LD must use an @graph");

for (const type of ["Person", "WebSite", "WebPage", "SoftwareApplication", "SoftwareSourceCode", "FAQPage"]) {
  invariant(graph.some((node) => node["@type"] === type), `JSON-LD is missing ${type}`);
}

const software = graph.find((node) => node["@type"] === "SoftwareApplication");
const webpage = graph.find((node) => node["@type"] === "WebPage");
const faq = graph.find((node) => node["@type"] === "FAQPage");
invariant(webpage.name === title, "the WebPage name must match the document title");
invariant(webpage.description === description, "the WebPage description must match the meta description");
invariant(webpage.dateModified === releasePublishedDate, "the WebPage modification date must match the release date");
invariant(software.name === "Aseprite Installer", "structured data describes the wrong software");
invariant(software.isAccessibleForFree === true, "the installer should be marked as freely accessible");
invariant(software.license.endsWith("/LICENSE"), "the installer MIT license URL is missing");
invariant(software.softwareVersion === releaseVersion, "the installer version is missing or incorrect");
invariant(software.releaseNotes === releaseUrl, "release notes must use the stable versioned release URL");
invariant(software.installUrl === "https://github.com/fmhun/aseprite-installer/releases/latest", "the generic install URL is incorrect");
invariant(
  JSON.stringify(software.downloadUrl) === JSON.stringify(expectedDownloadUrls),
  "structured download URLs do not match the exact v0.2.0 release asset allow-list",
);
invariant(
  JSON.stringify(software.operatingSystem) === JSON.stringify(expectedOperatingSystems),
  "structured operating-system support is incomplete or imprecise",
);
invariant(
  software.processorRequirements === expectedProcessorRequirements,
  "structured processor support is incomplete or imprecise",
);
invariant(software.datePublished === releasePublishedDate, "the structured release date is missing or incorrect");
const publishedDate = new Date(`${software.datePublished}T00:00:00Z`);
invariant(!Number.isNaN(publishedDate.valueOf()), "the release date must be a real calendar date");
invariant(publishedDate.toISOString().slice(0, 10) === software.datePublished, "the release date must be a real calendar date");
invariant(software.offers?.price === 0, "the installer offer must use a zero price");
invariant(software.offers?.priceCurrency === "USD", "the installer offer currency is missing");
invariant(software.offers?.url === releaseUrl, "the installer offer must point to the versioned release");
invariant(!software.aggregateRating && !software.review, "ratings or reviews must never be fabricated");
for (const question of faq.mainEntity ?? []) {
  invariant(html.includes(question.name), `the FAQ question is not visible: ${question.name}`);
  invariant(html.includes(question.acceptedAnswer?.text), `the FAQ answer is not visible: ${question.name}`);
}

const renderedLatestAssetUrls = [...new Set(
  [...html.matchAll(/href="(https:\/\/github\.com\/fmhun\/aseprite-installer\/releases\/latest\/download\/[^"]+)"/g)]
    .map((match) => match[1]),
)];
invariant(
  JSON.stringify(renderedLatestAssetUrls.sort()) === JSON.stringify(expectedLatestAssetUrls.sort()),
  "rendered download links do not match the release workflow asset allow-list",
);

invariant(robots.includes("User-agent: *"), "robots.txt has no general crawler rule");
invariant(robots.includes("Allow: /"), "robots.txt does not allow crawling");
invariant(robots.includes(`Sitemap: ${canonicalUrl}sitemap.xml`), "robots.txt references the wrong sitemap");
invariant(sitemap.includes(`<loc>${canonicalUrl}</loc>`), "the sitemap canonical URL is incorrect");
invariant(sitemap.includes(`<lastmod>${releasePublishedDate}</lastmod>`), "the sitemap release date is missing or incorrect");
await access(join(outputDirectory, "og.png"));

const localAssetUrls = [...html.matchAll(/(?:href|src)="(\/aseprite-installer\/[^"#?]+)"/g)]
  .map((match) => match[1]);
for (const assetUrl of new Set(localAssetUrls)) {
  const relativePath = assetUrl.slice(basePath.length);
  await access(join(outputDirectory, relativePath));
}

console.log("SEO validation passed: pre-rendered HTML, metadata, structured data, crawl files, and assets are coherent.");
