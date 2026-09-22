'use strict';
let credential = '', generation = 0, inspection = 0, selectedDevice = '', refreshing = false;
let fleet = {devices: [], commands: []};
const $ = id => document.getElementById(id);
const el = (tag, text, cls) => { const n = document.createElement(tag); if (text !== undefined) n.textContent = text; if (cls) n.className = cls; return n; };
const statuses = {queued:'대기 중',delivered:'실행 요청됨',completed:'완료',failed:'실패',uncertain:'결과 확인 필요',cancelled:'취소',pending:'승인 대기',approved:'승인됨',revoked:'관리 해제'};
const actions = {'deploy':'폴더 배포','scan':'변경 재검사','pending':'삭제 대기 조회','conflicts':'충돌 조회','history':'이전 버전 조회','resolve':'충돌 해결','restore':'이전 버전 복원','approve-deletion':'삭제 승인','set-job-enabled':'작업 제어','grant':'폴더 권한 변경','save-job':'작업 설정','trust-peer':'장치 신뢰 등록','create-folder':'폴더 등록'};
const terminal = status => ['completed','failed','uncertain','cancelled'].includes(status);
const stamp = time => time ? new Date(time*1000).toLocaleString() : '기록 없음';
const deviceName = id => fleet.devices.find(d => d.id === id)?.name || id.slice(0,12);
const badge = (text, state='') => el('span',text,`badge ${state}`);
let messageTimer;
function message(text, error=false) { clearTimeout(messageTimer); $('message').textContent=text; $('message').className=error?'error':''; $('message').hidden=false; messageTimer=setTimeout(()=>{$('message').hidden=true;},10000); }
function button(label, handler, cls='secondary') {
  const n=el('button',label,cls); n.type='button';
  n.addEventListener('click',async()=>{n.disabled=true;try{await handler();}catch(e){message(e.message,true);}finally{n.disabled=false;}});return n;
}
async function api(path, body) {
  const current=generation;
  const response=await fetch(path,{method:body?'POST':'GET',headers:{Authorization:`Bearer ${credential}`,...(body?{'Content-Type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,cache:'no-store'});
  if(current!==generation)throw new Error('화면이 잠겼습니다. 다시 로그인하세요.');
  const data=await response.json();
  if(!response.ok)throw new Error(data.error||`요청 실패 (${response.status})`);
  return data;
}
const action = body => api('/api/action',body);
function tab(name) {
  if(!['devices','folders','activity','guide'].includes(name))name='devices';
  document.querySelectorAll('.tab').forEach(n=>{n.hidden=n.id!==`tab-${name}`;});
  document.querySelectorAll('[data-tab]').forEach(n=>n.classList.toggle('active',n.dataset.tab===name));
}
window.addEventListener('hashchange',()=>tab(location.hash.slice(1)));
function sectionTitle(text) { const n=el('h3',text); return n; }
function selectDevices() {
  const devices=fleet.devices.filter(d=>d.state==='approved');
  const signature=JSON.stringify(devices.map(d=>[d.id,d.name]));
  for(const select of document.querySelectorAll('.device-select')){
    if(select.dataset.signature===signature)continue;
    const value=select.value;select.replaceChildren();const placeholder=el('option','승인된 장치 선택');placeholder.value='';select.append(placeholder);
    for(const d of devices){const opt=el('option',d.name);opt.value=d.id;select.append(opt);}
    select.value=value;select.dataset.signature=signature;
  }
}
function renderDevices() {
  $('device-list').replaceChildren();
  if(!fleet.devices.length)$('device-list').append(el('div','아직 등록된 장치가 없습니다. 장치 추가에서 첫 장치를 초대하세요.','empty'));
  for(const d of fleet.devices){
    const card=el('article',undefined,'device-card');const top=el('div',undefined,'card-top');
    top.append(el('span','▣','device-icon'),badge(d.state==='approved'?(d.online?'온라인':'오프라인'):statuses[d.state],d.state==='approved'?(d.online?'':'offline'):d.state));
    const report=d.report||{};const folders=report.folders||[];const jobs=report.jobs||[];
    card.append(top,el('h3',d.name),el('code',d.id));
    const stats=el('div',undefined,'device-stats');
    for(const [number,label] of [[report.folders_total??folders.length,'폴더'],[jobs.filter(j=>j.running).length,'실행 중']]){const item=el('span');item.append(el('strong',String(number)),document.createTextNode(label));stats.append(item);}
    card.append(stats,el('small',`최근 접속 ${stamp(d.last_seen)}${report.os?` · ${report.os}`:''}`));
    const controls=el('div',undefined,'card-actions');
    controls.append(button('장치 상세',()=>{selectedDevice=d.id;renderDeviceDetail();$('device-detail').scrollIntoView({behavior:'smooth',block:'start'});}));
    if(d.state!=='approved')controls.append(button('장치 승인',async()=>{
      if(!confirm(`${d.name}\n\n장치에서 표시된 지문과 일치하는지 확인하세요.\n${d.id}\n\n이 장치의 폴더와 작업을 중앙에서 관리하도록 승인할까요?`))return;
      await action({action:'set-device',device:d.id,approved:true});await refresh();message('장치를 승인했습니다. 에이전트를 실행하면 폴더를 연결할 수 있습니다.');
    }));
    card.append(controls);$('device-list').append(card);
  }
}
async function remote(device, operation, inspect=false) {
  const data=await action({action:'command',device,operation});const id=data.commands[0];
  message('명령을 등록했습니다. 적용 결과는 명령 기록에 표시됩니다.');await refresh();
  if(inspect){
    const requestInspection=++inspection;
    const command={id,device,operation,status:'queued',result:null};
    $('result-title').textContent=`${deviceName(device)} · ${actions[operation.action]||operation.action}`;
    $('result-content').replaceChildren(el('p','장치의 응답을 기다리고 있습니다. 오프라인이면 재접속한 뒤 실행됩니다.'));
    if(!$('result-dialog').open)$('result-dialog').showModal();
    const current=generation;
    for(let i=0;i<15;i++){
      await new Promise(resolve=>setTimeout(resolve,2000));
      if(current!==generation||requestInspection!==inspection||!$('result-dialog').open)return;
      const result=await api(`/api/command/${id}`);
      if(terminal(result.status)){showResult({...command,...result});await refresh();return;}
    }
    $('result-content').replaceChildren(el('p','아직 응답을 기다리는 중입니다. 창을 닫아도 명령은 유지됩니다. 명령 기록에서 결과를 확인하세요.'));
  }
}
function jobRow(d,job){
  const row=el('div',undefined,'job-row');
  row.append(el('strong',`${job.folder} · ${job.enabled?(job.running?'실행 중':'시작 대기'):'일시정지'}`),el('code',`${job.direction==='listen'?'수신 대기':'연결'} · ${job.address} · ${deviceName(job.peer)}`));
  row.append(el('small',`마지막 동기화 완료: ${stamp(job.last_success)}`));
  if(job.last_message)row.append(el('code',job.last_message));
  if(d.state==='approved')row.append(button(job.enabled?'일시정지':'다시 시작',()=>remote(d.id,{action:'set-job-enabled',id:job.id,enabled:!job.enabled})));
  return row;
}
function folderRow(d,folder){
  const row=el('div',undefined,'folder-row');row.append(el('h3',folder.folder),el('code',folder.root||''));
  if(folder.error){row.append(el('p',folder.error,'error-text'));return row;}
  const modes={'bidirectional':'양방향','send-only':'보내기 전용','receive-only':'받기 전용'};
  const meta=el('div',undefined,'folder-meta');
  for(const value of [`${folder.files}개 파일`,modes[folder.mode]||folder.mode,`삭제 대기 ${folder.pending_deletions}`,`동시 변경 ${folder.concurrent_paths}`])meta.append(el('span',value));row.append(meta);
  const controls=el('div',undefined,'card-actions');
  if(d.state==='approved'){
    for(const [label,type] of [['재검사','scan'],['삭제 승인 검토','pending'],['충돌 검토','conflicts']])controls.append(button(label,()=>remote(d.id,{action:type,folder:folder.folder},true)));
    controls.append(button('이전 버전',async()=>{const path=prompt('이전 버전을 조회할 파일의 상대 경로를 입력하세요. 예: 문서/note.txt');if(path)await remote(d.id,{action:'history',folder:folder.folder,path},true);}));
    controls.append(button('폴더 접근 회수',async()=>{
      const peer=prompt(`접근을 회수할 상대 장치 지문을 입력하세요.\n${(folder.peers||[]).map(id=>`${deviceName(id)}: ${id}`).join('\n')}`);
      if(!peer)return;if(!(folder.peers||[]).includes(peer))throw new Error('이 폴더에 등록된 장치 지문을 입력하세요.');
      if(confirm(`${deviceName(peer)}의 ${folder.folder} 접근을 회수할까요? 명령이 적용된 뒤 해당 장치와 파일 교환이 차단됩니다.`))await remote(d.id,{action:'grant',folder:folder.folder,peer,remove:true});
    }));
  }
  row.append(controls);return row;
}
function renderDeviceDetail(){
  const d=fleet.devices.find(d=>d.id===selectedDevice);$('device-detail').hidden=!d;if(!d)return;
  $('device-title').textContent=d.name;const content=$('device-content');content.replaceChildren(el('code',d.id,'fingerprint'));
  if(!d.online)content.append(el('p','현재 오프라인입니다. 아래 정보는 마지막 보고 내용이며, 새 명령은 재접속까지 대기합니다.','detail-note'));
  if(d.report?.error)content.append(el('p',d.report.error,'error-text'));
  if(d.report?.report_truncated)content.append(el('p',`폴더 요약이 커서 일부만 표시합니다. 전체 ${d.report.folders_total??'확인 불가'}개 중 ${d.report.folders?.length||0}개 표시 · 명령 제어는 유지됩니다.`,'detail-note'));
  if(d.state==='approved')content.append(button('중앙 관리 해제',async()=>{
    if(!confirm(`${d.name}의 중앙 관리를 해제할까요?\n\n대기 중 명령은 취소됩니다. 이미 실행된 동기화 작업과 다른 장치의 파일 접근 권한은 유지됩니다. 필요하면 먼저 작업을 정지하고 폴더 접근을 회수하세요.`))return;
    await action({action:'set-device',device:d.id,approved:false});await refresh();message('중앙 관리 권한을 해제했습니다.');
  },'danger'));
  content.append(sectionTitle('등록 폴더'));
  if(!d.report?.folders?.length)content.append(el('p','등록된 폴더 정보가 없습니다. 승인 후 폴더 연결에서 두 장치를 연결하세요.','muted'));
  for(const folder of d.report?.folders||[])content.append(folderRow(d,folder));
  content.append(sectionTitle('동기화 작업'));
  for(const job of d.report?.jobs||[])content.append(jobRow(d,job));
}
function renderConnections(){
  $('connections').replaceChildren();
  for(const d of fleet.devices){
    if(!d.report?.jobs?.length)continue;
    const panel=el('section',undefined,'panel');panel.append(el('h3',d.name));
    if(!d.online)panel.append(el('p','오프라인 · 마지막으로 보고된 작업 상태','detail-note'));
    for(const job of d.report.jobs)panel.append(jobRow(d,job));$('connections').append(panel);
  }
}
function renderCommands(){
  $('command-list').replaceChildren();
  for(const c of fleet.commands){
    const row=el('tr'),title=el('td',deviceName(c.device));title.append(el('small',`${actions[c.operation.action]||c.operation.action}${c.operation.folder||c.operation.job?.folder?` · ${c.operation.folder||c.operation.job.folder}`:''}`));
    const state=el('td');state.append(badge(statuses[c.status]||c.status,c.status));
    const result=el('td');result.append(button(terminal(c.status)?'결과 보기':'상세',()=>showResult(c)));
    if(c.status==='queued')result.append(button('취소',async()=>{await action({action:'cancel',id:c.id});await refresh();}));
    row.append(title,state,el('td',stamp(c.created)),result);$('command-list').append(row);
  }
  if(!fleet.commands.length){const row=el('tr'),cell=el('td','아직 요청한 명령이 없습니다.');cell.colSpan=4;row.append(cell);$('command-list').append(row);}
}
function revisionCard(command,path,item,historical){
  const row=el('div',undefined,'revision');const kind=item.content?.kind;
  row.append(el('strong',kind==='file'?'파일 내용':kind==='directory'?'폴더':'삭제 상태'),el('code',item.id));
  if(item.object_path)row.append(el('code',`장치에 보존된 파일: ${item.object_path}`));
  row.append(button(historical?'이 버전 복원':'이 버전으로 해결',async()=>{
    if(!confirm(`${path}\n선택한 버전을 새 변경으로 기록할까요? 다음 동기화에서 연결된 장치에도 적용됩니다.`))return;
    await remote(command.device,{action:historical?'restore':'resolve',folder:command.operation.folder,path,revision:item.id});$('result-dialog').close();
  }));return row;
}
function showResult(c){
  $('result-title').textContent=`${deviceName(c.device)} · ${actions[c.operation.action]||c.operation.action}`;
  const out=$('result-content');out.replaceChildren(badge(statuses[c.status]||c.status,c.status));
  if(c.status==='uncertain')out.append(el('p','실행 도중 장치가 종료되어 적용 여부를 확정할 수 없습니다. 장치의 폴더·작업·버전 이력을 확인한 뒤 필요할 때 새 명령을 요청하세요.'));
  if(!terminal(c.status))out.append(el('p','명령이 장치에 적용될 때까지 기다리고 있습니다. 명령 기록에서 상태를 확인하세요.'));
  else if(c.status==='completed'&&Array.isArray(c.result)){
    if(!c.result.length)out.append(el('p','표시할 항목이 없습니다.'));
    for(const entry of c.result){
      if(c.operation.action==='pending'){
        const row=el('div',undefined,'revision');row.append(el('strong',entry.path));
        row.append(button('이 삭제 승인',async()=>{
          if(!confirm(`${entry.path}\n삭제를 다른 장치에 전파하도록 승인할까요? 검토 이후 파일 상태가 바뀌면 승인이 거부됩니다.`))return;
          await remote(c.device,{action:'approve-deletion',folder:c.operation.folder,path:entry.path,expected:entry.versions});$('result-dialog').close();
        }));out.append(row);
      }else if(c.operation.action==='conflicts'){
        const group=el('section');group.append(el('h3',entry.path));for(const item of entry.revisions||[])group.append(revisionCard(c,entry.path,item,false));out.append(group);
      }else if(c.operation.action==='history')out.append(revisionCard(c,c.operation.path,entry,true));
      else out.append(el('pre',JSON.stringify(entry,null,2)));
    }
  }else if(c.result)out.append(el('pre',JSON.stringify(c.result,null,2)));
  out.append(el('code',`명령 ID: ${c.id}`));if(!$('result-dialog').open)$('result-dialog').showModal();
}
async function refresh(){
  if(refreshing)return;refreshing=true;
  try{
    fleet=await api('/api/status');
    const folders=fleet.devices.flatMap(d=>d.report?.folders||[]);
    $('online-count').textContent=String(fleet.devices.filter(d=>d.online&&d.state==='approved').length);
    $('total-count').textContent=`등록 장치 ${fleet.devices.length}대`;$('nav-count').textContent=String(fleet.devices.length);
    $('folder-count').textContent=String(fleet.devices.reduce((n,d)=>n+(d.report?.folders_total??d.report?.folders?.length??0),0));
    $('review-count').textContent=String(folders.reduce((n,f)=>n+(f.pending_deletions||0)+(f.concurrent_paths||0),0));
    $('queue-count').textContent=String(fleet.commands.filter(c=>!terminal(c.status)).length);
    $('updated').textContent=`${new Date().toLocaleTimeString()} 갱신`;
    renderDevices();renderDeviceDetail();renderCommands();renderConnections();selectDevices();
  }finally{refreshing=false;}
}
$('login-form').addEventListener('submit',async event=>{
  event.preventDefault();credential=$('token').value.trim();const submit=event.submitter;submit.disabled=true;
  try{await api('/api/health');await refresh();$('token').value='';$('login').hidden=true;$('workspace').hidden=false;$('lock').hidden=false;$('refresh').hidden=false;tab(location.hash.slice(1));}
  catch(e){credential='';message(e.message,true);}finally{submit.disabled=false;}
});
$('lock').addEventListener('click',()=>{
  generation++;credential='';selectedDevice='';fleet={devices:[],commands:[]};
  $('workspace').hidden=true;$('login').hidden=false;$('lock').hidden=true;$('refresh').hidden=true;$('updated').textContent='';
  for(const id of ['device-list','device-content','command-list','connections','result-content'])$(id).replaceChildren();
  $('result-dialog').close();$('invite-dialog').close();$('message').hidden=true;selectDevices();
});
$('refresh').addEventListener('click',()=>refresh().catch(e=>message(e.message,true)));
$('close-device').addEventListener('click',()=>{selectedDevice='';$('device-detail').hidden=true;});
$('open-invite').addEventListener('click',()=>{$('invite-help').hidden=true;$('invite-dialog').showModal();});
$('close-invite').addEventListener('click',()=>$('invite-dialog').close());
$('close-result').addEventListener('click',()=>$('result-dialog').close());
$('result-dialog').addEventListener('close',()=>{inspection++;});
$('invite-form').addEventListener('submit',async event=>{
  event.preventDefault();const form=new FormData(event.target);const submit=event.submitter;submit.disabled=true;
  try{
    const invitation=await action({action:'invite',name:form.get('name'),address:form.get('address').trim()});
    const url=URL.createObjectURL(new Blob([JSON.stringify(invitation,null,2)],{type:'application/json'}));
    const download=el('a');download.href=url;download.download='everywhere-invitation.json';document.body.append(download);download.click();download.remove();setTimeout(()=>URL.revokeObjectURL(url),1000);
    $('invite-help').hidden=false;message('초대 파일을 내려받았습니다. 등록할 장치에서 가져오세요.');
  }catch(e){message(e.message,true);}finally{submit.disabled=false;}
});
$('pair-form').addEventListener('submit',async event=>{
  event.preventDefault();const data=Object.fromEntries(new FormData(event.target));const submit=event.submitter;submit.disabled=true;
  try{
    if(data.listener===data.connector)throw new Error('서로 다른 두 장치를 선택하세요.');
    if(!confirm(`${deviceName(data.listener)} ↔ ${deviceName(data.connector)}\n폴더: ${data.folder}\n\n두 장치의 기존 폴더 내용을 선택한 방향으로 동기화할까요? 충돌 내용은 보존하고 삭제는 승인을 기다립니다.`))return;
    await action({action:'pair',...data});message('두 장치에 폴더 배포를 요청했습니다. 명령 기록에서 양쪽의 완료 여부를 확인하세요.');location.hash='activity';await refresh();
  }catch(e){message(e.message,true);}finally{submit.disabled=false;}
});
setInterval(()=>{if(credential&&!document.hidden)refresh().catch(e=>{$('updated').textContent='서버 응답 없음 · 기존 정보 표시';message(e.message,true);});},5000);
tab(location.hash.slice(1));
