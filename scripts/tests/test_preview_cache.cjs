const {test} = require('node:test');
const assert = require('node:assert/strict');
const {mkdtempSync, readFileSync, existsSync, rmSync} = require('node:fs');
const {tmpdir} = require('node:os');
const {join} = require('node:path');
const {checkCacheAccess} = require('../../.github/actions/preview-cache-check/index.js');
const env = {ACTIONS_CACHE_MODE: 'read', ACTIONS_RESULTS_URL: 'https://cache.example/', ACTIONS_RUNTIME_TOKEN: 'fixture'};

test('uses the real cache API and accepts explicit policy denial', async () => {
  await checkCacheAccess(env, async (url, options) => {
    assert.equal(url.pathname, '/twirp/github.actions.results.api.v1.CacheService/CreateCacheEntry');
    assert.equal(options.method, 'POST');
    assert.equal(options.headers.Authorization, 'Bearer fixture');
    return {status: 200, json: async () => ({ok: false, message: 'cache write denied: read-only token'})};
  });
});
for (const [name, result, status] of [
  ['writable token', {ok: true, signed_upload_url: 'https://example/upload'}, 200],
  ['cache collision', {ok: false, message: 'already exists'}, 200],
  ['invalid token', {code: 'unauthenticated'}, 401],
  ['service failure', {message: 'unavailable'}, 503],
]) {
  test(`rejects ${name} as proof of enforcement`, async () => {
    await assert.rejects(checkCacheAccess(env, async () => ({status, json: async () => result})));
  });
}
test('rejects absent or write-enabled mode before contacting the server', async () => {
  for (const mode of ['', 'write', 'write-only']) {
    await assert.rejects(checkCacheAccess({...env, ACTIONS_CACHE_MODE: mode}, () => assert.fail('unexpected request')));
  }
});

test('exports the shell marker only after verified server denial', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'fluxcope-cache-test-'));
  const file = join(directory, 'environment');
  try {
    for (const [mode, result] of [
      ['write', {message: 'cache write denied: read-only token'}],
      ['read', {ok: true, signed_upload_url: 'https://example/upload'}],
      ['read', {message: 'unavailable'}],
    ]) {
      await assert.rejects(checkCacheAccess({...env, ACTIONS_CACHE_MODE: mode, GITHUB_ENV: file},
        async () => ({status: 200, json: async () => result})));
      assert.equal(existsSync(file), false);
    }
    for (const mode of ['read', 'none']) {
      await checkCacheAccess({...env, ACTIONS_CACHE_MODE: mode, GITHUB_ENV: file},
        async () => ({status: 200, json: async () => ({message: 'cache write denied: policy'})}));
    }
    assert.equal(readFileSync(file, 'utf8'), 'FLUXCOPE_PREVIEW_CACHE_MODE=read\nFLUXCOPE_PREVIEW_CACHE_MODE=none\n');
  } finally {
    rmSync(directory, {recursive: true, force: true});
  }
});
