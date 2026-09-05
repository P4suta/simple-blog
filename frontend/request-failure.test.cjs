const test = require('node:test');
const assert = require('node:assert/strict');
const { RequestFailure, inquiryId, withInquiry } = require('./request-failure.ts');
test('only a bounded server correlation identifier is shown with a failure', () => {
  const id = 'a4134e5e-7044-4229-8e14-d7cf529b766c';
  assert.equal(withInquiry('Retry', new RequestFailure(503, '', id), '問い合わせID'), `Retry (問い合わせID: ${id})`);
  for (const unsafe of ['private-cookie', id + '\nprivate', '<script>', '', null]) {
    assert.equal(inquiryId(unsafe), null);
    assert.equal(withInquiry('Retry', new RequestFailure(503, '', unsafe)), 'Retry');
  }
  assert.equal(withInquiry('Offline', new TypeError('fetch')), 'Offline');
});
