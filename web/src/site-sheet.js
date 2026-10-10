function sheetC(p){
  const ins=p.kind==='service'?p.params:p.kind==='element'?p.attrs:[];
  const outs=p.kind==='service'?p.events:p.kind==='namespace'?p.elements.map(e=>'<'+p.prefix+':'+e+'>'):['assigns → '+(p.attrs[0]||'—')];
  const top=p.kind==='service'?p.env.map(e=>'env:'+e):[];
  const bot=p.kind==='service'?[p.grant==='none'?'no grant':p.grant.split(' ')[0]]:[];
  const n=Math.max(ins.length,outs.length,3),H=80+n*34,W=760,bx=240,bw=280,by=40,bh=H-80;
  let s=`<svg viewBox="0 0 ${W} ${H+40}" role="img" aria-label="Pin diagram for ${esc(p.id)}" font-family="JetBrains Mono, monospace">
   <rect x="${bx}" y="${by}" width="${bw}" height="${bh}" fill="rgba(127,208,255,.06)" stroke="#e8f1ff" stroke-width="1.5"/>
   <circle cx="${bx+16}" cy="${by+16}" r="5" fill="none" stroke="#e8f1ff"/>
   <text x="${bx+bw/2}" y="${by+bh/2-6}" text-anchor="middle" fill="#e8f1ff" font-size="20" font-family="Barlow Condensed, sans-serif" font-weight="600">${esc((p.kind==='element'?p.id:p.id).toUpperCase())}</text>
   <text x="${bx+bw/2}" y="${by+bh/2+16}" text-anchor="middle" fill="#9db8dc" font-size="11">${p.kind==='service'?'invoked service':p.kind==='namespace'?'extension namespace':'executable content'}</text>`;
  const step=(bh)/(n+1);
  ins.forEach((x,i)=>{const y=by+step*(i+1);s+=`<line x1="${bx-60}" y1="${y}" x2="${bx}" y2="${y}" stroke="#7fd0ff" stroke-width="1.5"/><circle cx="${bx-60}" cy="${y}" r="3.5" fill="#7fd0ff"/><text x="${bx-68}" y="${y+4}" text-anchor="end" fill="#e8f1ff" font-size="12">${esc(x)}</text><text x="${bx+8}" y="${y+4}" fill="#9db8dc" font-size="10">${i+1}</text>`});
  outs.forEach((x,i)=>{const y=by+step*(i+1);s+=`<line x1="${bx+bw}" y1="${y}" x2="${bx+bw+60}" y2="${y}" stroke="#ffd27a" stroke-width="1.5"/><path d="M${bx+bw+60} ${y-4} l7 4 l-7 4z" fill="#ffd27a"/><text x="${bx+bw+74}" y="${y+4}" fill="#e8f1ff" font-size="12">${esc(x.length>34?x.slice(0,33)+'…':x)}</text>`});
  top.forEach((x,i)=>{const X=bx+70+i*120;s+=`<line x1="${X}" y1="${by-26}" x2="${X}" y2="${by}" stroke="#e8f1ff" stroke-dasharray="3 3"/><text x="${X+6}" y="${by-14}" fill="#e8f1ff" font-size="11">${x}</text>`});
  bot.forEach((x,i)=>{const X=bx+bw/2;s+=`<line x1="${X}" y1="${by+bh}" x2="${X}" y2="${by+bh+24}" stroke="#e8f1ff" stroke-dasharray="3 3"/><text x="${X+6}" y="${by+bh+34}" fill="#e8f1ff" font-size="11">${esc(x)}</text>`});
  s+=`<text x="${bx-60}" y="20" fill="#9db8dc" font-size="11" font-family="Barlow Condensed, sans-serif" letter-spacing="2">${ins.length?(p.kind==='service'?'PARAMS IN':'ATTRIBUTES'):''}</text><text x="${bx+bw+20}" y="20" fill="#9db8dc" font-size="11" font-family="Barlow Condensed, sans-serif" letter-spacing="2">${p.kind==='service'?'EVENTS OUT':p.kind==='namespace'?'ELEMENTS':'RESULT'}</text></svg>`;
  const tb=p.kind==='service'?[['Part',p.id],['Version',p.ver],['World',p.wit],['Artifact',p.artifact]]:p.kind==='namespace'?[['Prefix',p.prefix],['Version',p.ver],['World',p.wit],['Artifact',p.artifact]]:[['Element',p.id],['Version',p.ver],['Namespace',p.ns],['Used in',p.uses+' documents']];
  return `<div class="sheet"><div class="ttl"><h3>${esc(p.kind==='element'?'<'+p.id+'>':p.id)}</h3><div class="sub">${esc(p.sum)}<br>${esc(p.uri)}</div></div>
   <div class="draw">${s}</div>
   <div class="snip"><b>Fit it</b><pre>${esc(snipOf(p))}</pre></div>
   <div class="tblock">${tb.map(([k,v])=>`<div><b>${k}</b>${esc(v)}</div>`).join('')}</div></div>`;
}

