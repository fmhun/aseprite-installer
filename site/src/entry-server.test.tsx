import { describe, expect, it } from "vitest";
import { render } from "./entry-server";

describe("static site rendering", () => {
  it("renders the complete landing content for crawlers before JavaScript runs", () => {
    const html = render();

    expect(html).toContain("<h1>");
    expect(html).toContain("Install <em>Aseprite</em>");
    expect(html).toContain("Choose your platform. Build locally.");
    expect(html).toContain('href="#install">Choose your platform');
    expect(html).toContain("Automatic detection runs locally");
    expect(html).not.toMatch(/detected ·|selected manually/i);
    expect(html).toContain("macOS 15.2+");
    expect(html).toContain("Windows 11");
    expect(html).toContain("Linux x86_64");
    expect(html).toContain("AppImage");
    expect(html).toContain("/releases/latest/download/Aseprite-Installer-macOS-arm64.dmg");
    expect(html).toContain("/releases/latest/download/Aseprite-Installer-Windows-x64-setup.exe");
    expect(html).toContain("/releases/latest/download/Aseprite-Installer-Linux-x86_64.AppImage");
    expect(html).toContain("/releases/latest/download/SHA256SUMS");
    expect(html).not.toContain("Aseprite-Installer-macOS-Universal");
    expect(html).toContain("How it works");
    expect(html).toContain("OPEN SOURCE");
    expect(html).toContain("The essentials.");
    expect(html).toContain("Which installer should I choose?");
    expect(html).toContain("On macOS, choose the DMG for your Mac’s architecture.");
    expect(html).toContain("Does the installer distribute Aseprite?");
    expect(html).toContain("official Aseprite source and Skia release assets");
    expect(html).toContain("What happens to an existing copy?");
    expect(html).toContain("Installer-managed builds can keep a backup");
    expect(html).toContain("Aseprite EULA");
  });
});
