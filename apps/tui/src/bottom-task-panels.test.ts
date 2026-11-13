import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { visibleWidth, setKeybindings } from "@earendil-works/pi-tui";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { ScheduledManagerComponent } from "../lib/coding-agent/src/modes/interactive/components/scheduled-manager.js";
import { ShellActivityPanel } from "../lib/coding-agent/src/modes/interactive/components/shell-activity-panel.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { KeybindingsManager } from "../lib/coding-agent/src/core/keybindings.js";
setKeybindings(new KeybindingsManager());
const plain = (lines: string[]) => stripVTControlCharacters(lines.join("\n"));
test("Scheduled compact viewport keeps selected task and navigation; left backs out", () => {
  initTheme("dark"); let closes = 0; let rows = 6;
  const tasks = Array.from({length: 30}, (_, i) => ({job: {id: String(i),sessionId:"s",status:"active",source:"heartbeat",createdAt:String(i).padStart(2,"0"),prompt:`task-${i}`,schedule:{expression:"every 1h"},runCount:0}})) as any;
  const panel = new ScheduledManagerComponent({getScheduledTasks:()=>tasks,getRows:()=>rows,onClose:()=>closes++,onAction:async()=>{},requestRender:()=>{}});
  panel.render(40);
  for(let i=0;i<29;i++) panel.handleInput("\x1b[B");
  assert.match(plain(panel.render(40)),/› active · task-29/);
  assert.doesNotMatch(plain(panel.render(40)),/task-0\n/);
  panel.handleInput("\r"); assert.match(plain(panel.render(40)),/Pause scheduled task/);
  panel.handleInput("\x1b[D"); assert.equal(closes,0); assert.match(plain(panel.render(40)),/task-29/);
  panel.handleInput("\x1b[D"); assert.equal(closes,1);
  for(rows of [1,2,3,6,12]) for(const width of [0,1,20,40,100]) {
    const lines=panel.render(width); assert.ok(lines.length<=rows); assert.ok(lines.every(l=>visibleWidth(l)<=width));
  }
});
test("Shell detail left returns to list and then closes; small viewports retain exit hint", () => {
 initTheme("dark");let closes=0;let rows=6;
 const panel=new ShellActivityPanel({getTasks:()=>[{id:"one",command:"build",state:"running",cwd:"/p",output:"a\nb\nc"}],getRows:()=>rows,getError:()=>undefined,onClose:()=>closes++,requestRender:()=>{}});
 panel.render(40);panel.handleInput("\r");panel.handleInput("\x1b[D");assert.equal(closes,0);
 panel.handleInput("\x1b[D");assert.equal(closes,1);
 for(rows of [1,2,3,6]) {panel.handleInput("\r");const lines=panel.render(40);assert.ok(lines.length<=rows);assert.match(plain(lines),/esc back/);}
});
test("production opening uses bottom anchored half-height overlays, not full-pane wrappers", async () => {
 initTheme("dark");const shown:any[]=[];
 const fake:any={ui:{terminal:{rows:24},showOverlay:(component:any,options:any)=>{shown.push({component,options});return {hide(){},focus(){}};},requestRender(){}},shellActivity:{status:"ready",items:[]},closeShellActivity(){},refreshScheduledCatalog:async()=>{},getScopedScheduledTasks:()=>[],scheduledManagerActionState:{},manageScheduledTask:async()=>{},closeScheduledManager(){},scheduleScheduledManagerRefresh(){}};
 (InteractiveMode.prototype as any).showShellActivity.call(fake);
 await (InteractiveMode.prototype as any).showScheduledManager.call(fake);
 for(const {component,options} of shown){assert.equal(options.anchor,"bottom-center");assert.equal(options.width,"100%");assert.equal(options.suspendFullscreenMouse,true);assert.ok(component.render(40).length<=12);fake.ui.terminal.rows=8;assert.ok(component.render(40).length<=4);}
});
test("short Scheduled action views keep task identity and failed status; invisible actions cannot execute", async () => {
 initTheme("dark");let rows=4;let calls=0;
 const job:any={id:"a",sessionId:"s",status:"active",source:"heartbeat",prompt:"Important backup",lastError:"disk full",createdAt:"0",schedule:{expression:"every 1h"},runCount:0};
 const panel=new ScheduledManagerComponent({getScheduledTasks:()=>[{job}],getRows:()=>rows,onClose(){},onAction:async()=>{calls++;},requestRender(){}});
 assert.match(plain(panel.render(40)),/error · Important backup/);
 panel.handleInput("\r");assert.match(plain(panel.render(40)),/Scheduled · Important backup/);
 rows=8;assert.match(plain(panel.render(40)),/Last error: disk full/);
 rows=2;panel.handleInput("\r");assert.equal(calls,0);
});
