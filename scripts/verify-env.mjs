import { spawnSync } from 'node:child_process';
import process from 'node:process';

function commandVersion(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    windowsHide: true,
  });
  return result.status === 0 ? result.stdout + result.stderr : '';
}

function commandAvailable(command) {
  const locator = process.platform === 'win32' ? 'where' : 'which';
  return spawnSync(locator, [command], {
    encoding: 'utf8',
    windowsHide: true,
  }).status === 0;
}

const javaOutput = commandVersion('java', ['-version']);
const javaMajor = Number(javaOutput.match(/version\s+"(\d+)/)?.[1] ?? 0);
const androidSdk = Boolean(process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT);
const msbuild = Boolean(process.env.MSBUILD_EXE) || commandAvailable('msbuild');
const dotnet = commandAvailable('dotnet');
const pwsh = commandAvailable('pwsh');

const checks = {
  node: Number(process.versions.node.split('.')[0]) >= 22,
  androidSdk,
  java17: javaMajor === 17,
  msbuild,
  dotnet,
  pwsh,
};

const lines = Object.entries(checks).map(
  ([name, ok]) => `${name}: ${ok ? 'ok' : 'missing'}`,
);
const nativeSmoke = androidSdk && javaMajor === 17 && msbuild && dotnet && pwsh;
lines.push(`nativeSmoke: ${nativeSmoke ? 'ready' : 'blocked'}`);

console.log(lines.join('\n'));
process.exitCode = checks.node ? 0 : 1;
