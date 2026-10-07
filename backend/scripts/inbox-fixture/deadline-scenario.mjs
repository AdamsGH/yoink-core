import assert from 'node:assert/strict';

// Page one finishes after 3s; page two's own request deadline is 4s. The
// centrally owned 6s job must cancel page two first, on both admitted attempts.
export async function deadlineContracts(c) {
  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_jobs WHERE state IN ('ready','running')")) === '0', 'drain work before outer deadline gate');
  await c.stop(); await c.launch({ deadlines: true }); await c.ready();
  const token = await c.login(42);
  await c.db.batch("INSERT INTO insight_user_settings(user_id,github_token) VALUES(42,'fixture-read') ON CONFLICT(user_id) DO UPDATE SET github_token=excluded.github_token");
  c.trusted.deadlineRequests.length = 0;
  c.trusted.deadlineStars = true;
  try {
    await c.request('/api/v1/inbox/gh_stars/sync', { token, method: 'POST', expected: 202 });
    const job = await c.db.query("SELECT max(id) FROM inbox_jobs WHERE kind='stars_sync'");
    await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE id=${job}`)) === 'failed', 'outer deadlines consume both configured attempts', 30000);
    assert.equal(await c.db.query(`SELECT attempts,coalesce(lease_token,''),coalesce(lease_until,'') FROM inbox_jobs WHERE id=${job}`), '2||');
    await c.waitFor(() => c.trusted.deadlineRequests.filter(row => row.page === '2' && row.closedAfter !== null).length === 2, 'both actual paginated reads cancelled');
    const secondPages = c.trusted.deadlineRequests.filter(row => row.page === '2');
    assert.equal(secondPages.length, 2);
    for (const page of secondPages) assert(page.closedAfter < 3900, 'outer job cancels page two before its independent 4s adapter deadline');
    c.trusted.deadlineStars = false;
    await c.request('/api/v1/inbox/gh_stars/sync', { token, method: 'POST', expected: 202 });
    await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE kind='stars_sync' AND id>${job} ORDER BY id DESC LIMIT 1`)) === 'done', 'later eligible job completes after terminal outer deadline');
    assert.equal(c.trusted.deadlineRequests.filter(row => row.page === '2').length, 2, 'terminal job does not dispatch again');
  } finally {
    c.trusted.deadlineStars = false;
    await c.stop(); await c.launch(); await c.ready();
  }
}
