'use strict';
let credential = '';
const $ = (id) => document.getElementById(id);
const el = (tag, text, className) => { const node = document.createElement(tag); if (text !== undefined) node.textContent = text; if (className) node.className = className; return node; };
function message(text, error = false) { $('message').textContent = text; $('message').className = error ? 'error' : ''; $('message').hidden = false; }
async function api(path, body) {
  const response = await fetch(path, { method: body ? 'POST' : 'GET', headers: { Authorization: `Bearer ${credential}`, ...(body ? {'Content-Type': 'application/json'} : {}) }, body: body ? JSON.stringify(body) : undefined, cache: 'no-store' });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || `요청 실패 (${response.status})`);
  return data;
}
const command = (body) => api('/api/command', body);
function action(label, handler) {
  const button = el('button', label, 'quiet');
  button.type = 'button';
  button.addEventListener('click', async () => { button.disabled = true; try { await handler(); } catch (error) { message(error.message, true); } finally { button.disabled = false; } });
  return button;
}
async function revisions(folder, path, items, historical) {
  $('details').hidden = false;
  $('detail-title').textContent = `${folder} / ${path} · ${historical ? '이전 버전' : '충돌 선택'}`;
  $('detail-content').replaceChildren();
  if (!items.length) $('detail-content').append(el('p', '표시할 버전이 없습니다.', 'muted'));
  for (const item of items) {
    const card = el('div', undefined, 'revision');
    const label = item.content.kind === 'file' ? '파일 내용' : item.content.kind === 'directory' ? '폴더' : '삭제 상태';
    card.append(el('strong', label), el('code', item.id));
    if (item.object_path) card.append(el('code', `보존된 파일: ${item.object_path}`));
    card.append(action(historical ? '이 버전 복원' : '이 버전으로 해결', async () => {
      if (!window.confirm('선택한 상태를 새 변경으로 기록합니다. 다음 동기화에서 연결된 장비에도 적용됩니다. 계속할까요?')) return;
      await command({action: historical ? 'restore' : 'resolve', folder, path, revision: item.id});
      message('변경을 기록했습니다. 다음 동기화에서 전파됩니다.');
      $('details').hidden = true;
      await refresh();
    }));
    $('detail-content').append(card);
  }
  $('details').scrollIntoView({behavior:'smooth', block:'start'});
}
async function refresh() {
  const data = await api('/api/status');
  $('identity').textContent = data.identity;
  $('version').textContent = data.version;
  $('folder-list').replaceChildren();
  if (!data.folders.length) $('folder-list').append(el('div', '아직 등록된 폴더가 없습니다. 아래에서 첫 폴더를 연결하세요.', 'empty'));
  for (const folder of data.folders) {
    const card = el('article', undefined, 'folder');
    const top = el('div', undefined, 'folder-top');
    const name = el('div'); name.append(el('h3', folder.folder), el('code', folder.root || '', 'path'));
    top.append(el('div', '▰', 'folder-icon'), name);
    card.append(top);
    if (folder.error) { card.append(el('p', folder.error, 'error-text')); $('folder-list').append(card); continue; }
    const metrics = el('div', undefined, 'metrics');
    for (const [value, label] of [[folder.files,'파일'], [folder.pending_deletions,'삭제 승인 대기'], [folder.concurrent_paths,'동시 변경'], [folder.peers.length,'승인 장비']]) {
      const metric = el('span'); metric.append(el('b', String(value)), document.createTextNode(label)); metrics.append(metric);
    }
    const actions = el('div', undefined, 'actions');
    actions.append(action('변경 재검사', async () => { const result = await command({action:'scan',folder:folder.folder}); message(`${result.changed}개 변경 기록 · ${result.pending_deletions}개 삭제 승인 대기`); await refresh(); }));
    actions.append(action('충돌 확인', async () => {
      const conflicts = await command({action:'conflicts',folder:folder.folder});
      $('details').hidden = false; $('detail-title').textContent = `${folder.folder} · 충돌`; $('detail-content').replaceChildren();
      if (!conflicts.length) $('detail-content').append(el('p','해결할 충돌이 없습니다.','muted'));
      for (const conflict of conflicts) $('detail-content').append(action(conflict.path, () => revisions(folder.folder, conflict.path, conflict.revisions, false)));
      $('details').scrollIntoView({behavior:'smooth'});
    }));
    actions.append(action('이전 버전 복구', async () => { const path = window.prompt('폴더 안의 상대 경로를 입력하세요. 예: 문서/note.txt'); if (!path) return; await revisions(folder.folder,path,await command({action:'history',folder:folder.folder,path}),true); }));
    card.append(metrics, actions); $('folder-list').append(card);
  }
}
$('login-form').addEventListener('submit', async (event) => { event.preventDefault(); credential = $('token').value.trim(); try { await refresh(); $('token').value = ''; $('login').hidden = true; $('workspace').hidden = false; $('lock').hidden = false; $('message').hidden = true; } catch (error) { credential = ''; message(error.message,true); } });
$('lock').addEventListener('click', () => { credential = ''; $('workspace').hidden = true; $('folder-list').replaceChildren(); $('detail-content').replaceChildren(); $('identity').textContent = ''; $('login').hidden = false; $('lock').hidden = true; $('message').hidden = true; });
$('refresh').addEventListener('click', () => refresh().catch(error => message(error.message,true)));
$('close-details').addEventListener('click', () => { $('details').hidden = true; });
$('folder-form').addEventListener('submit', async (event) => { event.preventDefault(); const form = new FormData(event.target); try { await command({action:'create-folder',folder:form.get('folder'),root:form.get('root'),mode:form.get('mode')}); event.target.reset(); await refresh(); message('폴더를 등록했습니다. 장비 승인 후 동기화를 연결하세요.'); } catch(error) { message(error.message,true); } });
