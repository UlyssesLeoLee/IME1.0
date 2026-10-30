'use strict';
const crypto = require('node:crypto');

const languageMap = Object.freeze({
  rust:'rust', python:'python', typescript:'typescript', typescriptreact:'typescript',
  powershell:'powershell', c:'c', asm:'assembly', nasm:'assembly',
  assembly:'assembly', 'machine-code':'machine-code'
});
const extensionMap = Object.freeze({
  rs:'rust',py:'python',pyw:'python',ts:'typescript',tsx:'typescript',
  ps1:'powershell',psm1:'powershell',c:'c',h:'c',s:'assembly',asm:'assembly',nasm:'assembly',
  hex:'machine-code'
});
const localSnippets = Object.freeze({
  rust: [
    ['fnm','main function','fn main() {\n    \n}'],
    ['fori','iterator loop','for item in items.iter() {\n    \n}'],
    ['iflet','Option match','if let Some(value) = option {\n    \n}'],
    ['match','match expression','match value {\n    _ => {}\n}'],
    ['impl','implementation','impl Type {\n    \n}']
  ],
  python: [
    ['def','function','def function():\n    pass'],
    ['defmain','main entry',"def main():\n    pass\n\nif __name__ == '__main__':\n    main()"],
    ['fori','for loop','for item in items:\n    pass'],
    ['try','exception handling','try:\n    pass\nexcept Exception:\n    raise']
  ],
  typescript:[
    ['fn','arrow function','const name = (value: unknown): void => {\n  \n};'],
    ['interface','interface','interface Name {\n  id: string;\n}'],
    ['fori','for loop','for (const item of items) {\n  \n}'],
    ['async','async function','async function name(): Promise<void> {\n  \n}']
  ],
  powershell:[
    ['func','function','function Invoke-Name {\n    param()\n}'],
    ['foreach','pipeline foreach','$items | ForEach-Object {\n    $_\n}'],
    ['try','try/catch','try {\n    \n} catch {\n    throw\n}']
  ],
  c:[
    ['main','main','int main(void) {\n    return 0;\n}'],
    ['fori','for loop','for (size_t i = 0; i < count; ++i) {\n    \n}'],
    ['struct','structure','struct Name {\n    int value;\n};']
  ],
  assembly:[
    ['mov','x86-64 MOV (Intel syntax)','mov rax, rbx'],
    ['prologue','x86-64 stack frame','push rbp\nmov rbp, rsp'],
    ['ret','return','ret']
  ],
  'machine-code':[
    ['nop','x86-64 NOP byte','90'], ['ret','x86-64 RET byte','C3'],
    ['int3','x86-64 breakpoint byte','CC'], ['syscall','x86-64 SYSCALL bytes','0F 05']
  ]
});
function hash(text) { return crypto.createHash('sha256').update(text, 'utf8').digest('hex'); }
function detectLanguage(languageId, filename, lock='auto') {
  if (lock !== 'auto' && Object.hasOwn(localSnippets, lock)) return lock;
  if (languageMap[languageId]) return languageMap[languageId];
  const match = /\.([^.\\/]+)$/.exec(filename || '');
  return match ? (extensionMap[match[1].toLowerCase()] || 'unknown') : 'unknown';
}
function predict(language, prefix, max=5) {
  const key=prefix.trim().toLowerCase();
  if(!key || key.length>128 || !Object.hasOwn(localSnippets,language)) return [];
  return localSnippets[language].filter(x=>x[0].startsWith(key)).slice(0, Math.min(max, 20))
    .map(([trigger,label,text])=>({ trigger,label,text,origin:'local' }));
}
function parseAIJson(raw) {
  if(typeof raw!=='string' || raw.length>1048576) throw new Error('AI_RESULT_INVALID');
  const clean=raw.trim().replace(/^```(?:json)?\s*/i,'').replace(/\s*```$/,'');
  const data=JSON.parse(clean);
  if(!data || typeof data!=='object' || Array.isArray(data)) throw new Error('AI_RESULT_INVALID');
  if('fullFile' in data && (typeof data.fullFile !== 'string' || data.fullFile.length>500000)) throw new Error('AI_RESULT_INVALID');
  if('insertCandidates' in data && (!Array.isArray(data.insertCandidates) || data.insertCandidates.length>8 ||
       data.insertCandidates.some(x=>typeof x!=='string'||x.length>100000))) throw new Error('AI_RESULT_INVALID');
  return {fullFile: data.fullFile, insertCandidates:data.insertCandidates||[],
          explanation:typeof data.explanation==='string'?data.explanation.slice(0,2000):''};
}
function requireUnchanged(snapshot, current) {
  if(snapshot.uri!==current.uri || snapshot.version!==current.version || snapshot.hash!==hash(current.text))
    throw new Error('STALE_DOCUMENT');
  return true;
}
function resolveEffort(effort, capability) {
  if(effort==='auto' || capability!=='supported') return undefined;
  if(!['low','medium','high'].includes(effort)) throw new Error('INVALID_EFFORT');
  return effort;
}
function getLastWord(beforeCursor) { return (/[a-zA-Z_$][a-zA-Z0-9_$]{0,63}$/.exec(beforeCursor)||[''])[0]; }
module.exports={languageMap,localSnippets,hash,detectLanguage,predict,parseAIJson,requireUnchanged,resolveEffort,getLastWord};
