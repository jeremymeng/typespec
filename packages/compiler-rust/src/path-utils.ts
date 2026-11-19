// Port of the subset of `packages/compiler/src/core/path-utils.ts` needed by the
// multi-file host. Kept self-contained so this package does not depend on the
// TS compiler at runtime.

const directorySeparator = "/";
const relativePathSegmentRegExp = /(?:\/\/)|(?:^|\/)\.\.?(?:$|\/)/;

function isVolumeCharacter(charCode: number) {
  return (charCode >= 97 && charCode <= 122) || (charCode >= 65 && charCode <= 90);
}

function getFileUrlVolumeSeparatorEnd(url: string, start: number) {
  const ch0 = url.charCodeAt(start);
  if (ch0 === 58 /* : */) return start + 1;
  if (ch0 === 37 /* % */ && url.charCodeAt(start + 1) === 51 /* 3 */) {
    const ch2 = url.charCodeAt(start + 2);
    if (ch2 === 97 || ch2 === 65) return start + 3;
  }
  return -1;
}

function getEncodedRootLength(path: string): number {
  if (!path) return 0;
  const ch0 = path.charCodeAt(0);
  if (ch0 === 47 || ch0 === 92) {
    if (path.charCodeAt(1) !== ch0) return 1;
    const p1 = path.indexOf(ch0 === 47 ? "/" : "\\", 2);
    if (p1 < 0) return path.length;
    return p1 + 1;
  }
  if (isVolumeCharacter(ch0) && path.charCodeAt(1) === 58) {
    const ch2 = path.charCodeAt(2);
    if (ch2 === 47 || ch2 === 92) return 3;
    if (path.length === 2) return 2;
  }
  const schemeEnd = path.indexOf("://");
  if (schemeEnd !== -1) {
    const authorityStart = schemeEnd + 3;
    const authorityEnd = path.indexOf(directorySeparator, authorityStart);
    if (authorityEnd !== -1) {
      const scheme = path.slice(0, schemeEnd);
      const authority = path.slice(authorityStart, authorityEnd);
      if (
        scheme === "file" &&
        (authority === "" || authority === "localhost") &&
        isVolumeCharacter(path.charCodeAt(authorityEnd + 1))
      ) {
        const volumeSeparatorEnd = getFileUrlVolumeSeparatorEnd(path, authorityEnd + 2);
        if (volumeSeparatorEnd !== -1) {
          if (path.charCodeAt(volumeSeparatorEnd) === 47) return ~(volumeSeparatorEnd + 1);
          if (volumeSeparatorEnd === path.length) return ~volumeSeparatorEnd;
        }
      }
      return ~(authorityEnd + 1);
    }
    return ~path.length;
  }
  return 0;
}

export function getRootLength(path: string) {
  const rootLength = getEncodedRootLength(path);
  return rootLength < 0 ? ~rootLength : rootLength;
}

export function normalizeSlashes(path: string): string {
  return path.includes("\\") ? path.replace(/\\/g, directorySeparator) : path;
}

function hasTrailingDirectorySeparator(path: string) {
  const last = path.charCodeAt(path.length - 1);
  return path.length > 0 && (last === 47 || last === 92);
}

function removeTrailingDirectorySeparator(path: string) {
  return hasTrailingDirectorySeparator(path) ? path.substring(0, path.length - 1) : path;
}

function ensureTrailingDirectorySeparator(path: string) {
  return hasTrailingDirectorySeparator(path) ? path : path + directorySeparator;
}

export function getDirectoryPath(path: string): string {
  path = normalizeSlashes(path);
  const rootLength = getRootLength(path);
  if (rootLength === path.length) return path;
  path = removeTrailingDirectorySeparator(path);
  return path.slice(0, Math.max(rootLength, path.lastIndexOf(directorySeparator)));
}

export function getBaseFileName(path: string): string {
  path = normalizeSlashes(path);
  const rootLength = getRootLength(path);
  if (rootLength === path.length) return "";
  path = removeTrailingDirectorySeparator(path);
  return path.slice(Math.max(getRootLength(path), path.lastIndexOf(directorySeparator) + 1));
}

export function getAnyExtensionFromPath(path: string): string {
  const baseFileName = getBaseFileName(path);
  const extensionIndex = baseFileName.lastIndexOf(".");
  return extensionIndex >= 0 ? baseFileName.substring(extensionIndex).toLowerCase() : "";
}

function pathComponents(path: string, rootLength: number) {
  const root = path.substring(0, rootLength);
  const rest = path.substring(rootLength).split(directorySeparator);
  if (rest.length && !rest[rest.length - 1]) rest.pop();
  return [root, ...rest];
}

function reducePathComponents(components: readonly string[]) {
  if (!components.some((x) => x !== undefined)) return [];
  const reduced = [components[0]];
  for (let i = 1; i < components.length; i++) {
    const component = components[i];
    if (!component || component === ".") continue;
    if (component === "..") {
      if (reduced.length > 1) {
        if (reduced[reduced.length - 1] !== "..") {
          reduced.pop();
          continue;
        }
      } else if (reduced[0]) continue;
    }
    reduced.push(component);
  }
  return reduced;
}

function getPathFromPathComponents(components: readonly string[]) {
  if (components.length === 0) return "";
  const root = components[0] && ensureTrailingDirectorySeparator(components[0]);
  return root + components.slice(1).join(directorySeparator);
}

export function joinPaths(path: string, ...paths: (string | undefined)[]): string {
  if (path) path = normalizeSlashes(path);
  for (let relativePath of paths) {
    if (!relativePath) continue;
    relativePath = normalizeSlashes(relativePath);
    path =
      !path || getRootLength(relativePath) !== 0
        ? relativePath
        : ensureTrailingDirectorySeparator(path) + relativePath;
  }
  return path;
}

export function normalizePath(path: string): string {
  path = normalizeSlashes(path);
  if (!relativePathSegmentRegExp.test(path)) return path;
  const simplified = path.replace(/\/\.\//g, "/").replace(/^\.\//, "");
  if (simplified !== path) {
    path = simplified;
    if (!relativePathSegmentRegExp.test(path)) return path;
  }
  const normalized = getPathFromPathComponents(
    reducePathComponents(pathComponents(path, getRootLength(path))),
  );
  return normalized && hasTrailingDirectorySeparator(path)
    ? ensureTrailingDirectorySeparator(normalized)
    : normalized;
}

export function resolvePath(path: string, ...paths: (string | undefined)[]): string {
  return normalizePath(
    paths.some((x) => x !== undefined) ? joinPaths(path, ...paths) : normalizeSlashes(path),
  );
}
