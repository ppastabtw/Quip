"""Render a self-contained benchmark dashboard."""

from __future__ import annotations

import json
from typing import Any, Mapping


def render_dashboard(summary: Mapping[str, Any]) -> str:
    data = json.dumps(summary, ensure_ascii=False, separators=(",", ":")).replace(
        "</", r"<\/"
    )
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Quip model benchmark</title>
<style>
:root {{
  color-scheme: light;
  --bg:#f5f4ef;--surface:#fff;--ink:#171a18;--muted:#6d716e;--line:#d8dad5;
  --grid:#e9eae6;--accent:#17634f;--accent-soft:#deede7;--bad:#a43f38;
  --freesolo:#242825;--openai:#278169;--anthropic:#b9653c;--google:#3c6fc9;
  --xai:#6c55ac;--backboard:#278169;
}}
@media (prefers-color-scheme:dark) {{
  :root {{
    color-scheme:dark;--bg:#111412;--surface:#191d1b;--ink:#eef1ed;--muted:#a8aea9;
    --line:#353b37;--grid:#292e2b;--accent:#70c8a8;--accent-soft:#17392e;
    --bad:#ed8d85;--freesolo:#d7ddd8;--openai:#70c8a8;--anthropic:#e49b71;
    --google:#86aaed;--xai:#ad99e8;--backboard:#70c8a8;
  }}
}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--ink);font-family:Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{width:min(1460px,calc(100% - 32px));margin:auto;padding:28px 0 48px}}
header{{display:flex;justify-content:space-between;align-items:end;gap:24px;padding-bottom:18px;border-bottom:1px solid var(--line)}}
h1{{margin:0;font-size:clamp(28px,4vw,46px);font-weight:650;letter-spacing:-.045em}}
.run-meta{{color:var(--muted);font-size:12px;text-align:right;line-height:1.7}}
.stats{{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));border-bottom:1px solid var(--line)}}
.stat{{padding:20px 18px 18px 0}}.stat+.stat{{padding-left:18px;border-left:1px solid var(--line)}}
.stat-label{{color:var(--muted);font-size:11px;letter-spacing:.08em;text-transform:uppercase}}
.stat-value{{margin-top:6px;font-size:clamp(21px,3vw,31px);font-weight:650;letter-spacing:-.035em;font-variant-numeric:tabular-nums}}
.stat-note{{margin-top:3px;color:var(--muted);font-size:12px;overflow-wrap:anywhere}}
section{{margin-top:34px}}.section-head{{display:flex;align-items:baseline;justify-content:space-between;gap:16px;margin-bottom:13px}}
h2{{margin:0;font-size:18px;font-weight:650;letter-spacing:-.015em}}.section-note{{color:var(--muted);font-size:12px;text-align:right}}
.chart-shell{{position:relative;background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:14px}}
#scatter{{display:block;width:100%;height:455px}}.grid-line{{stroke:var(--grid);stroke-width:1}}.axis-label,.tick-label{{fill:var(--muted);font-size:11px}}
.point{{stroke:var(--surface);stroke-width:2}}.point-label{{fill:var(--ink);font-size:11px;font-weight:600;pointer-events:none}}
.frontier{{fill:none;stroke:var(--accent);stroke-width:2;stroke-dasharray:5 5}}
.tooltip{{position:absolute;display:none;pointer-events:none;max-width:260px;padding:9px 11px;border:1px solid var(--line);border-radius:7px;background:var(--surface);box-shadow:0 8px 30px rgba(0,0,0,.13);font-size:12px;line-height:1.45}}
.legend{{display:flex;flex-wrap:wrap;gap:14px;margin-top:10px;color:var(--muted);font-size:12px}}.legend-item{{display:inline-flex;align-items:center;gap:6px}}.swatch{{width:9px;height:9px;border-radius:50%;background:var(--swatch)}}
.bars{{display:grid;gap:9px}}.bar-row{{display:grid;grid-template-columns:minmax(130px,240px) minmax(130px,1fr) 62px;gap:12px;align-items:center}}
.bar-label{{font-size:13px;font-weight:600;overflow-wrap:anywhere}}.bar-track{{height:12px;background:var(--grid);border-radius:2px;overflow:hidden}}.bar-fill{{height:100%;width:var(--width);background:var(--color)}}.bar-value{{text-align:right;font-size:13px;font-variant-numeric:tabular-nums}}
.provider-dot{{display:inline-block;width:7px;height:7px;margin-right:7px;border-radius:50%;background:var(--provider-color)}}
.table-wrap{{overflow-x:auto;border-top:1px solid var(--line)}}table{{width:100%;border-collapse:collapse;font-size:13px}}th,td{{padding:11px 10px;border-bottom:1px solid var(--line);text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}}
th{{color:var(--muted);font-size:11px;font-weight:650;letter-spacing:.045em;text-transform:uppercase;cursor:pointer;user-select:none}}th:first-child,td:first-child,th:nth-child(2),td:nth-child(2){{text-align:left}}tbody tr:hover{{background:var(--accent-soft)}}.model-cell{{font-weight:650}}.heatmap th:first-child,.heatmap td:first-child{{position:sticky;left:0;background:var(--bg);z-index:1}}.heat{{background:color-mix(in srgb,var(--accent) var(--heat),transparent)}}.bad{{color:var(--bad)}}
@media(max-width:760px){{main{{width:min(100% - 20px,1460px);padding-top:18px}}header,.section-head{{align-items:start;flex-direction:column}}.section-head{{gap:4px}}.section-note,.run-meta{{text-align:left}}.stats{{grid-template-columns:repeat(2,minmax(0,1fr))}}.stat:nth-child(3){{border-left:0}}.stat:nth-child(n+3){{border-top:1px solid var(--line)}}#scatter{{height:390px}}.bar-row{{grid-template-columns:minmax(105px,150px) 1fr 54px;gap:8px}}}}
</style>
</head>
<body>
<main>
<header><h1>Quip benchmark</h1><div class="run-meta" id="run-meta"></div></header>
<div class="stats" aria-label="Benchmark highlights">
  <div class="stat"><div class="stat-label">Models</div><div class="stat-value" id="model-count"></div></div>
  <div class="stat"><div class="stat-label">Examples</div><div class="stat-value" id="example-count"></div></div>
  <div class="stat"><div class="stat-label">Highest success</div><div class="stat-value" id="best-score"></div><div class="stat-note" id="best-model"></div></div>
  <div class="stat"><div class="stat-label">Lowest mean latency</div><div class="stat-value" id="fastest-time"></div><div class="stat-note" id="fastest-model"></div></div>
</div>
<section><div class="section-head"><h2>Quality versus latency</h2><div class="section-note">Upper left is best. Dashed line: Pareto frontier.</div></div><div class="chart-shell" id="chart-shell"><svg id="scatter" role="img" aria-labelledby="scatter-title scatter-desc"><title id="scatter-title">Model quality versus response latency</title><desc id="scatter-desc">Higher task success and lower mean latency are better.</desc></svg><div class="tooltip" id="tooltip" role="status"></div><div class="legend" id="legend"></div></div></section>
<section><div class="section-head"><h2>Task success</h2><div class="section-note">Correct output, edit decision, and schema.</div></div><div class="bars" id="quality-bars"></div></section>
<section><div class="section-head"><h2>Category performance</h2></div><div class="table-wrap"><table class="heatmap" id="category-table"></table></div></section>
<section><div class="section-head"><h2>Model comparison</h2></div><div class="table-wrap"><table id="comparison-table"></table></div></section>
</main>
<script>
const benchmark={data};
const models=benchmark.models||[];
const providerKey=m=>m.transport==="freesolo"?"freesolo":(m.provider||"backboard");
const colors={{freesolo:"var(--freesolo)",openai:"var(--openai)",anthropic:"var(--anthropic)",google:"var(--google)",xai:"var(--xai)",backboard:"var(--backboard)"}};
const colorFor=m=>colors[providerKey(m)]||"var(--accent)";
const pct=v=>Number.isFinite(v)?`${{(v*100).toFixed(1)}}%`:"n/a";
const ms=v=>Number.isFinite(v)?`${{Math.round(v).toLocaleString()}} ms`:"n/a";
const usd=v=>Number.isFinite(v)?`$${{v.toFixed(6)}}`:"n/a";
document.getElementById("run-meta").innerHTML=`${{new Date(benchmark.created_at).toLocaleString(undefined,{{year:"numeric",month:"short",day:"numeric",hour:"numeric",minute:"2-digit",timeZone:"UTC",timeZoneName:"short"}})}}<br>${{String(benchmark.dataset).split(/[\\/]/).pop()}}`;
document.getElementById("model-count").textContent=models.length;
document.getElementById("example-count").textContent=benchmark.examples;
const qualityRank=[...models].sort((a,b)=>b.metrics.overall_success-a.metrics.overall_success||a.metrics.mean_latency_ms-b.metrics.mean_latency_ms);
const latencyRank=models.filter(m=>Number.isFinite(m.metrics.mean_latency_ms)).sort((a,b)=>a.metrics.mean_latency_ms-b.metrics.mean_latency_ms);
const best=qualityRank[0],fastest=latencyRank[0];
document.getElementById("best-score").textContent=best?pct(best.metrics.overall_success):"n/a";
document.getElementById("best-model").textContent=best?best.label:"No completed models";
document.getElementById("fastest-time").textContent=fastest?ms(fastest.metrics.mean_latency_ms):"n/a";
document.getElementById("fastest-model").textContent=fastest?fastest.label:"No latency data";
const svg=document.getElementById("scatter"),shell=document.getElementById("chart-shell"),tooltip=document.getElementById("tooltip");
const node=(name,attrs={{}})=>{{const n=document.createElementNS("http://www.w3.org/2000/svg",name);Object.entries(attrs).forEach(([k,v])=>n.setAttribute(k,v));return n}};
const frontier=items=>items.filter(c=>!items.some(o=>o!==c&&o.metrics.overall_success>=c.metrics.overall_success&&o.metrics.mean_latency_ms<=c.metrics.mean_latency_ms&&(o.metrics.overall_success>c.metrics.overall_success||o.metrics.mean_latency_ms<c.metrics.mean_latency_ms))).sort((a,b)=>a.metrics.mean_latency_ms-b.metrics.mean_latency_ms);
function drawScatter(){{
  svg.replaceChildren();const w=Math.max(680,svg.clientWidth||900),h=svg.clientHeight||455,m={{top:28,right:150,bottom:54,left:62}},pw=w-m.left-m.right,ph=h-m.top-m.bottom;svg.setAttribute("viewBox",`0 0 ${{w}} ${{h}}`);
  const points=models.filter(x=>Number.isFinite(x.metrics.mean_latency_ms)&&Number.isFinite(x.metrics.overall_success));if(!points.length)return;
  const maxX=Math.max(...points.map(x=>x.metrics.mean_latency_ms))*1.08||1,minScore=Math.min(...points.map(x=>x.metrics.overall_success)),minY=Math.max(0,Math.floor((minScore-.08)*10)/10);
  const x=v=>m.left+v/maxX*pw,y=v=>m.top+(1-v)/(1-minY||1)*ph;
  for(let i=0;i<=5;i++){{const xv=maxX*i/5,xp=x(xv);svg.appendChild(node("line",{{x1:xp,y1:m.top,x2:xp,y2:h-m.bottom,class:"grid-line"}}));const t=node("text",{{x:xp,y:h-m.bottom+22,"text-anchor":"middle",class:"tick-label"}});t.textContent=Math.round(xv).toLocaleString();svg.appendChild(t)}}
  for(let i=0;i<=5;i++){{const yv=minY+(1-minY)*i/5,yp=y(yv);svg.appendChild(node("line",{{x1:m.left,y1:yp,x2:w-m.right,y2:yp,class:"grid-line"}}));const t=node("text",{{x:m.left-10,y:yp+4,"text-anchor":"end",class:"tick-label"}});t.textContent=`${{Math.round(yv*100)}}%`;svg.appendChild(t)}}
  const xt=node("text",{{x:m.left+pw/2,y:h-10,"text-anchor":"middle",class:"axis-label"}});xt.textContent="Mean response latency, ms";svg.appendChild(xt);
  const yt=node("text",{{x:16,y:m.top+ph/2,"text-anchor":"middle",class:"axis-label",transform:`rotate(-90 16 ${{m.top+ph/2}})`}});yt.textContent="Overall success";svg.appendChild(yt);
  const edge=frontier(points);if(edge.length>1)svg.appendChild(node("path",{{d:edge.map((model,i)=>`${{i?"L":"M"}} ${{x(model.metrics.mean_latency_ms)}} ${{y(model.metrics.overall_success)}}`).join(" "),class:"frontier"}}));
  const labels=[];
  points.forEach(model=>{{const cx=x(model.metrics.mean_latency_ms),cy=y(model.metrics.overall_success),p=node("circle",{{cx,cy,r:7,fill:colorFor(model),class:"point","aria-label":`${{model.label}}, ${{pct(model.metrics.overall_success)}} success, ${{ms(model.metrics.mean_latency_ms)}}`}});p.addEventListener("mouseenter",()=>{{tooltip.innerHTML=`<strong>${{model.label}}</strong><br>${{pct(model.metrics.overall_success)}} overall success<br>${{ms(model.metrics.mean_latency_ms)}} mean latency<br>${{pct(model.metrics.unnecessary_edit_rate)}} unnecessary edits`;tooltip.style.display="block";tooltip.style.left=`${{Math.min(shell.clientWidth-tooltip.offsetWidth-8,Math.max(8,cx+20))}}px`;tooltip.style.top=`${{Math.max(8,cy-tooltip.offsetHeight/2)}}px`}});p.addEventListener("mouseleave",()=>tooltip.style.display="none");svg.appendChild(p);const offsets=[-12,15,-31,34,-50,53],offset=offsets.find(candidate=>!labels.some(label=>Math.abs(label.x-cx)<180&&Math.abs(label.y-(cy+candidate))<25))??-12,labelY=cy+offset;labels.push({{x:cx,y:labelY}});const t=node("text",{{x:cx+11,y:labelY,class:"point-label"}});t.textContent=model.label;svg.appendChild(t)}});
}}
drawScatter();let resizeTimer;window.addEventListener("resize",()=>{{clearTimeout(resizeTimer);resizeTimer=setTimeout(drawScatter,100)}});
const providers=[...new Set(models.map(providerKey))];
document.getElementById("legend").innerHTML=providers.length>1?providers.map(p=>`<span class="legend-item"><span class="swatch" style="--swatch:${{colors[p]||"var(--accent)"}}"></span>${{p}}</span>`).join(""):"";
document.getElementById("quality-bars").innerHTML=qualityRank.map(m=>`<div class="bar-row"><div class="bar-label"><span class="provider-dot" style="--provider-color:${{colorFor(m)}}"></span>${{m.label}}</div><div class="bar-track" aria-label="${{m.label}} success ${{pct(m.metrics.overall_success)}}"><div class="bar-fill" style="--width:${{m.metrics.overall_success*100}}%;--color:${{colorFor(m)}}"></div></div><div class="bar-value">${{pct(m.metrics.overall_success)}}</div></div>`).join("");
const categories=[...new Set(models.flatMap(m=>Object.keys(m.metrics.categories||{{}})))].sort(),categoryTable=document.getElementById("category-table");
const categoryLabels={{human_grammar_correction:"Grammar",lexical_normalization:"Normalization",social_keep:"Keep"}};
categoryTable.innerHTML=`<thead><tr><th>Model</th>${{categories.map(c=>`<th>${{categoryLabels[c]||c.replaceAll("_"," ")}}</th>`).join("")}}</tr></thead><tbody>${{qualityRank.map(m=>`<tr><td class="model-cell"><span class="provider-dot" style="--provider-color:${{colorFor(m)}}"></span>${{m.label}}</td>${{categories.map(c=>{{const v=m.metrics.categories?.[c]?.success_rate;return`<td class="heat" style="--heat:${{Number.isFinite(v)?Math.round(v*72):0}}%">${{pct(v)}}</td>`}}).join("")}}</tr>`).join("")}}</tbody>`;
const allColumns=[["label","Model",m=>m.label,"text"],["transport","Route",m=>m.transport,"text"],["success","Success",m=>m.metrics.overall_success,"pct"],["decode","Decode",m=>m.metrics.decode_success,"pct"],["edits","Unneeded edits",m=>m.metrics.unnecessary_edit_rate,"pct"],["schema","Schema",m=>m.metrics.schema_validity,"pct"],["mean","Mean ms",m=>m.metrics.mean_latency_ms,"ms"],["p95","P95 ms",m=>m.runtime.p95_latency_ms,"ms"],["cost","Est. USD",m=>m.runtime.estimated_cost_usd,"usd"],["errors","Errors",m=>m.runtime.errors,"number"]];
const columns=allColumns.filter(c=>(c[0]!=="transport"||new Set(models.map(m=>m.transport)).size>1)&&(c[0]!=="cost"||models.some(m=>Number.isFinite(m.runtime.estimated_cost_usd))));
let sortKey="success",sortDirection=-1;const table=document.getElementById("comparison-table");
function renderTable(){{const col=columns.find(c=>c[0]===sortKey),sorted=[...models].sort((a,b)=>{{const av=col[2](a),bv=col[2](b);if(typeof av==="string")return av.localeCompare(bv)*sortDirection;if(!Number.isFinite(av))return 1;if(!Number.isFinite(bv))return-1;return(av-bv)*sortDirection}});table.innerHTML=`<thead><tr>${{columns.map(c=>`<th data-key="${{c[0]}}">${{c[1]}}${{sortKey===c[0]?(sortDirection===1?" ▲":" ▼"):""}}</th>`).join("")}}</tr></thead><tbody>${{sorted.map(m=>`<tr>${{columns.map((c,i)=>{{const v=c[2](m),display=c[3]==="pct"?pct(v):c[3]==="ms"?(Number.isFinite(v)?Math.round(v).toLocaleString():"n/a"):c[3]==="usd"?usd(v):v;return`<td class="${{i===0?"model-cell":""}} ${{c[0]==="errors"&&v?"bad":""}}">${{i===0?`<span class="provider-dot" style="--provider-color:${{colorFor(m)}}"></span>`:""}}${{display}}</td>`}}).join("")}}</tr>`).join("")}}</tbody>`;table.querySelectorAll("th").forEach(h=>h.addEventListener("click",()=>{{const next=h.dataset.key;if(sortKey===next)sortDirection*=-1;else{{sortKey=next;sortDirection=next==="label"||next==="transport"?1:-1}}renderTable()}}))}}renderTable();
</script>
</body>
</html>
"""
