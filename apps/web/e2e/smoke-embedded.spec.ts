import { expect, test } from './fixtures/unifiedBinary';
import { expectScenarioControlsRequireAuthentication } from './fixtures/scenarioControl';

test('serves hashed assets and authenticated status from one binary', async ({
  request,
  unifiedBinary,
}) => {
  const htmlResponse = await request.get(`${unifiedBinary.baseUrl}/`);
  expect(htmlResponse.status()).toBe(200);
  const html = await htmlResponse.text();
  const asset = html.match(/\/assets\/[^"']+\.js/)?.[0];

  expect(asset).toBeTruthy();
  if (asset === undefined) throw new Error('embedded HTML has no JavaScript asset');
  expect((await request.get(`${unifiedBinary.baseUrl}${asset}`)).status()).toBe(200);
  expect((await request.get(`${unifiedBinary.baseUrl}/api/v1/status`)).status()).toBe(401);
  expect(
    (
      await request.get(`${unifiedBinary.baseUrl}/api/v1/status`, {
        headers: { Authorization: `Bearer ${unifiedBinary.credential}` },
      })
    ).status(),
  ).toBe(200);
  if (!unifiedBinary.releasePackage) {
    await expectScenarioControlsRequireAuthentication(request, unifiedBinary);
  }
});
