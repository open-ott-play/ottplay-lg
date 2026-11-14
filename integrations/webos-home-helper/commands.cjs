'use strict';
// Prints reviewed manual commands only. Never executes SSH, Luna or TV commands.
const SERVICE='luna://com.openottplay.homehelper.control/';
function command(action,args=[]){let payload={};
  if(action==='trial'){if(args.length!==1)throw Error('trial SECONDS');payload.trialSeconds=Number(args[0]);if(!Number.isInteger(payload.trialSeconds)||payload.trialSeconds<1||payload.trialSeconds>120)throw Error('trial SECONDS: 1..120');}
  else if(action==='enable'){if(args.length!==2||args[0]!=='--accept-tested-controls'||!/^[a-f0-9]{32}$/.test(args[1]||''))throw Error('enable --accept-tested-controls TRIAL_SESSION');payload={acceptTestedControls:true,testedTrialSession:args[1]};}
  else if(!['status','pause','ensure'].includes(action)||args.length)throw Error('Use status, trial SECONDS, pause, enable --accept-tested-controls SESSION, or ensure');
  return "luna-send-pub -n 1 "+SERVICE+action+" '"+JSON.stringify(payload)+"'";
}
module.exports={command};
if(require.main===module){try{console.log(command(process.argv[2],process.argv.slice(3)));}catch(e){console.error(e.message);process.exitCode=1;}}
