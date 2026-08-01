import { describe, expect, it } from "vitest";
import { render } from "./entry-server";

describe("static site rendering", () => {
  it("renders the complete landing content for crawlers before JavaScript runs", () => {
    const html = render();

    expect(html).toContain("<h1>");
    expect(html).toContain("Install <em>Aseprite</em>");
    expect(html).toContain("Aseprite Installer is a free, MIT-licensed desktop utility");
    expect(html).toContain("Choose your installer");
    expect(html).toContain("Windows 11");
    expect(html).toContain("AppImage");
    expect(html).not.toContain("/releases/latest/download/");
    expect(html).toContain("How it works");
    expect(html).toContain("OPEN SOURCE");
    expect(html).toContain("The essentials.");
    expect(html).toContain("Aseprite EULA");
  });
});
