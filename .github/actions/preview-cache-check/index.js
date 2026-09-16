// Run before candidate code. A toolkit's local save guard is not enforcement.
const {createHash} = require('node:crypto');
async function checkCacheAccess(env = process.env, request = fetch) {
  const mode = env.ACTIONS_CACHE_MODE;
  if (!['read', 'none'].includes(mode)) {
    throw new Error('GitHub did not provide the required read-only cache mode');
  }
  const base = env.ACTIONS_RESULTS_URL;
  const token = env.ACTIONS_RUNTIME_TOKEN;
  if (!base || !token) throw new Error('Cannot verify GitHub cache token enforcement');
  const response = await request(new URL('/twirp/github.actions.results.api.v1.CacheService/CreateCacheEntry', base), {
    method: 'POST',
    headers: {Authorization: `Bearer ${token}`, 'Content-Type': 'application/json'},
    body: JSON.stringify({key: `fluxcope-denial-probe-${env.GITHUB_RUN_ID}-${env.GITHUB_JOB}-${env.GITHUB_RUN_ATTEMPT}`,
      version: createHash('sha256').update('fluxcope-preview-cache-denial-v1').digest('hex')}),
    signal: AbortSignal.timeout(30000),
  });
  const result = await response.json();
  // Match the backend's documented policy-denial prefix, including its HTTP
  // 200 / ok:false form. An outage or invalid token is not a successful probe.
  const message = result.message || result.msg || '';
  if (!message.startsWith('cache write denied:') || result.ok || result.signed_upload_url || result.signedUploadUrl) {
    throw new Error(`Cache server did not explicitly deny writes (HTTP ${response.status})`);
  }
  console.log(`Server denied cache writes with cache-mode=${mode}`);
}

module.exports = {checkCacheAccess};
if (require.main === module) {
  checkCacheAccess().catch(error => { console.error(error.message); process.exitCode = 1; });
}
