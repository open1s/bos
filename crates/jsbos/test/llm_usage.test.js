const test = require('ava');

test('LlmUsage - can use as type annotation', t => {
  const usage = {
    promptTokens: 100,
    completionTokens: 50,
    totalTokens: 150,
  };
  
  t.is(usage.promptTokens, 100);
  t.is(usage.completionTokens, 50);
  t.is(usage.totalTokens, 150);
});

test('LlmUsage - with prompt tokens details', t => {
  const usage = {
    promptTokens: 100,
    completionTokens: 50,
    totalTokens: 150,
    promptTokensDetails: {
      audioTokens: 10,
      cachedTokens: 20,
    },
  };
  
  t.is(usage.promptTokensDetails.audioTokens, 10);
  t.is(usage.promptTokensDetails.cachedTokens, 20);
});

test('LlmUsage - optional fields can be undefined', t => {
  const usage = {
    promptTokens: 100,
    completionTokens: 50,
    totalTokens: 150,
  };
  
  t.is(usage.promptTokensDetails, undefined);
});

test('LlmUsage - zero values', t => {
  const usage = {
    promptTokens: 0,
    completionTokens: 0,
    totalTokens: 0,
  };
  
  t.is(usage.promptTokens, 0);
  t.is(usage.completionTokens, 0);
  t.is(usage.totalTokens, 0);
});

test('LlmUsage - max values', t => {
  const usage = {
    promptTokens: 2147483647,
    completionTokens: 2147483647,
    totalTokens: 4294967294,
  };
  
  t.is(usage.promptTokens, 2147483647);
  t.is(usage.completionTokens, 2147483647);
  t.is(usage.totalTokens, 4294967294);
});

test('PromptTokensDetails - can use', t => {
  const details = {
    audioTokens: 5,
    cachedTokens: 10,
  };
  
  t.is(details.audioTokens, 5);
  t.is(details.cachedTokens, 10);
});

test('PromptTokensDetails - optional fields', t => {
  const details = {};
  
  t.is(details.audioTokens, undefined);
  t.is(details.cachedTokens, undefined);
});