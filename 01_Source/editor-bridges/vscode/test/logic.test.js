'use strict';
const {test}=require('node:test');
const assert=require('node:assert/strict');
const {hash,detectLanguage,predict,parseAIJson,requireUnchanged,resolveEffort,getLastWord}=require('../src/logic');
test('seven local language profiles',()=>{
  for(const lang of ['rust','python','typescript','powershell','c','assembly','machine-code'])
    assert.ok(predict(lang,Object.entries({rust:'fnm',python:'def',typescript:'fn',powershell:'func',c:'main',assembly:'mov','machine-code':'nop'})
      .find(([key])=>key===lang)[1]).length>0);
});
test('manual language lock wins',()=>assert.equal(detectLanguage('python','x.py','rust'),'rust'));
test('typescriptreact detected',()=>assert.equal(detectLanguage('typescriptreact','X.tsx'),'typescript'));
test('unknown syntax conservatively unknown',()=>assert.equal(detectLanguage('plaintext','Untitled'),'unknown'));
test('prefix result safe',()=>assert.ok(predict('rust','fnm')[0].text.includes('fn main')));
test('no candidate on empty input',()=>assert.deepEqual(predict('rust',''),[]));
test('AI result validated',()=>assert.equal(parseAIJson('{"fullFile":"abc","insertCandidates":["def"]}').fullFile,'abc'));
test('invalid AI output rejected',()=>assert.throws(()=>parseAIJson('{"insertCandidates":[25]}')));
test('document hash version guards',()=>{
  const s={uri:'file:///a',version:1,hash:hash('abc')};
  assert.equal(requireUnchanged(s,{uri:s.uri,version:1,text:'abc'}),true);
  assert.throws(()=>requireUnchanged(s,{uri:s.uri,version:2,text:'abc'}));
});
test('unknown model effort omitted',()=>assert.equal(resolveEffort('high','unknown'),undefined));
test('capability effort emitted',()=>assert.equal(resolveEffort('high','supported'),'high'));
test('last word',()=>assert.equal(getLastWord('let x = fnm'),'fnm'));
