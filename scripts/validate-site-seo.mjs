import { access, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const outputDirectory = join(repositoryRoot, "dist-site");
const canonicalUrl = "https://fmhun.github.io/aseprite-installer/";
const basePath = "/aseprite-installer/";

function invariant(condition, message) {
  if (!condition) throw new Error(`SEO validation failed: ${message}`);
}

function occurrences(value, pattern) {
  return [...value.matchAll(pattern)].length;
}

const [html, robots, sitemap, htmlStats, sourceStyles] = await Promise.all([
  readFile(join(outputDirectory, "index.html"), "utf8"),
  readFile(join(outputDirectory, "robots.txt"), "utf8"),
  readFile(join(outputDirectory, "sitemap.xml"), "utf8"),
  stat(join(outputDirectory, "index.html")),
  readFile(join(repositoryRoot, "site/src/styles.css"), "utf8"),
]);

invariant(htmlStats.size < 150_000, "the initial HTML exceeds the 150 KB crawl budget");
invariant(html.includes('data-prerendered="true"'), "the React root is not pre-rendered");
invariant(!html.includes("<!--app-html-->"), "the pre-render placeholder remains in the build");
invariant(/<main\s+[^>]*id="main"[^>]*>/i.test(html), "the initial HTML does not contain the main landmark");
invariant(occurrences(html, /<h1(?:\s|>)/g) === 1, "the page must expose exactly one H1");
invariant(html.includes("Install <em>Aseprite</em>"), "the H1 is missing from the initial HTML");
invariant(html.includes("Aseprite Installer is a free, MIT-licensed desktop utility"), "the factual product description is missing");
invariant(html.includes("Does the installer distribute Aseprite?"), "the FAQ is missing from the initial HTML");
invariant(html.includes("Aseprite remains subject to its own EULA"), "the Aseprite license distinction is missing");
invariant(html.includes('href="https://github.com/fmhun/aseprite-installer/releases/latest"'), "the latest release link is missing");
invariant(!html.includes("/releases/latest/download/"), "the page guesses a release asset name");
invariant(!html.includes("18 GB"), "the obsolete disk-space requirement remains");

const title = html.match(/<title>([^<]+)<\/title>/)?.[1] ?? "";
const description = html.match(/<meta\s+name="description"\s+content="([^"]+)"/s)?.[1] ?? "";
invariant(title.length >= 30 && title.length <= 60, "the title should be 30–60 characters");
invariant(description.length >= 120 && description.length <= 160, "the meta description should be 120–160 characters");
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
invariant(software.name === "Aseprite Installer", "structured data describes the wrong software");
invariant(software.isAccessibleForFree === true, "the installer should be marked as freely accessible");
invariant(software.license.endsWith("/LICENSE"), "the installer MIT license URL is missing");
invariant(software.softwareVersion === "0.1.0", "the installer version is missing or incorrect");
invariant(software.offers?.price === 0, "the installer offer must use a zero price");
invariant(software.offers?.priceCurrency === "USD", "the installer offer currency is missing");
invariant(!software.aggregateRating && !software.review, "ratings or reviews must never be fabricated");
for (const question of faq.mainEntity ?? []) {
  invariant(html.includes(question.name), `the FAQ question is not visible: ${question.name}`);
  invariant(html.includes(question.acceptedAnswer?.text), `the FAQ answer is not visible: ${question.name}`);
}

invariant(robots.includes("User-agent: *"), "robots.txt has no general crawler rule");
invariant(robots.includes("Allow: /"), "robots.txt does not allow crawling");
invariant(robots.includes(`Sitemap: ${canonicalUrl}sitemap.xml`), "robots.txt references the wrong sitemap");
invariant(sitemap.includes(`<loc>${canonicalUrl}</loc>`), "the sitemap canonical URL is incorrect");
await access(join(outputDirectory, "og.png"));

const localAssetUrls = [...html.matchAll(/(?:href|src)="(\/aseprite-installer\/[^"#?]+)"/g)]
  .map((match) => match[1]);
for (const assetUrl of new Set(localAssetUrls)) {
  const relativePath = assetUrl.slice(basePath.length);
  await access(join(outputDirectory, relativePath));
}

console.log("SEO validation passed: pre-rendered HTML, metadata, structured data, crawl files, and assets are coherent.");
