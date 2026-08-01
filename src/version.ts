export function normalizeVersion(version: string | null): number[] {
  if (!version) return [];
  return version
    .replace(/^v/, "")
    .split("-", 1)[0]
    .split(".")
    .slice(0, 4)
    .map((part) => Number.parseInt(part, 10))
    .map((part) => (Number.isFinite(part) ? part : 0));
}

interface Prerelease {
  rank: number;
  sequence: number;
}

const STABLE_RANK = 3;
const prereleaseRanks: Record<string, number> = {
  rc: 2,
  beta: 1,
  alpha: 0,
};

function parsePrerelease(version: string | null): Prerelease {
  const suffix = version?.replace(/^v/, "").split("-", 2)[1]?.toLowerCase();
  if (!suffix) return { rank: STABLE_RANK, sequence: 0 };

  const match = /^(alpha|beta|rc)(?:[.-]?(\d+))?/.exec(suffix);
  if (!match) {
    return {
      rank: 0,
      sequence: Number.parseInt(suffix.match(/\d+/)?.[0] ?? "0", 10),
    };
  }

  return {
    rank: prereleaseRanks[match[1]],
    sequence: Number.parseInt(match[2] ?? "0", 10),
  };
}

export function compareVersions(
  left: string | null,
  right: string | null,
): number {
  const a = normalizeVersion(left);
  const b = normalizeVersion(right);
  const length = Math.max(a.length, b.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return Math.sign(difference);
  }

  const leftPrerelease = parsePrerelease(left);
  const rightPrerelease = parsePrerelease(right);
  const rankDifference = leftPrerelease.rank - rightPrerelease.rank;
  if (rankDifference !== 0) return Math.sign(rankDifference);

  return Math.sign(leftPrerelease.sequence - rightPrerelease.sequence);
}
