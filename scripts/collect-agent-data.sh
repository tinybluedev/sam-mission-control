#!/bin/bash
# Collect cron jobs and context usage from all fleet agents
DB_HOST="10.64.0.2"; DB_PORT="30306"; DB_USER="root"; DB_PASS='Nw1026039$'; DB_NAME="quantum_memory"
mysql_cmd() { mysql -h "$DB_HOST" -P "$DB_PORT" --skip-ssl -u "$DB_USER" -p"$DB_PASS" "$DB_NAME" -N -e "$1" 2>/dev/null; }

AGENTS=$(mysql_cmd "SELECT agent_name, tailscale_ip FROM mc_fleet_status WHERE status='online'")
while IFS=$'\t' read -r name ip; do
    [ -z "$name" ] && continue
    user="papasmurf"; [ "$name" = "almalinux9" ] && user="nick"
    
    # Collect cron jobs
    CRONS=$(timeout 8 ssh -o ConnectTimeout=3 -o BatchMode=yes "$user@$ip" "cat ~/.openclaw/cron/jobs.json 2>/dev/null" 2>/dev/null)
    if [ -n "$CRONS" ]; then
        echo "$CRONS" | python3 -c "
import json,sys,subprocess
data=json.load(sys.stdin); agent='$name'
for j in data.get('jobs',[]):
    sid=j.get('id','')[:64]; jname=j.get('name','').replace(chr(39),''); desc=j.get('description','').replace(chr(39),'')
    sched=j.get('schedule',{}); kind=sched.get('kind',''); val=str(sched.get('cron',sched.get('at',sched.get('every',''))))[:128]
    en=1 if j.get('enabled',True) else 0; tgt=j.get('sessionTarget','main')[:64]
    print(f'REPLACE INTO mc_agent_crons (agent_name,cron_id,name,schedule_kind,schedule_value,enabled,session_target,description,last_collected_at) VALUES (%s,%s,%s,%s,%s,%s,%s,%s,NOW());'%(repr(agent),repr(sid),repr(jname[:256]),repr(kind),repr(val),en,repr(tgt),repr(desc[:512])))
" 2>/dev/null | while read -r sql; do mysql_cmd "$sql"; done
    fi
    
    # Collect context
    CTX=$(timeout 8 ssh -o ConnectTimeout=3 -o BatchMode=yes "$user@$ip" '
        SD=~/.openclaw/agents/main/sessions; C=$(ls $SD 2>/dev/null|wc -l); L=$(ls -t $SD 2>/dev/null|head -1)
        T=0; [ -n "$L" ] && T=$(wc -c < $SD/$L 2>/dev/null)
        M=$(python3 -c "import json,os;c=json.load(open(os.path.expanduser(\"~/.openclaw/openclaw.json\")));print(c.get(\"agents\",{}).get(\"defaults\",{}).get(\"contextTokens\",1000000))" 2>/dev/null||echo 1000000)
        echo "$C $T $M"
    ' 2>/dev/null)
    if [ -n "$CTX" ]; then
        read sc bytes mx <<< "$CTX"
        et=$((bytes/4)); pct=$(python3 -c "print(round($et/$mx*100,1))" 2>/dev/null||echo 0)
        mysql_cmd "INSERT INTO mc_agent_context (agent_name,session_count,context_tokens_used,context_tokens_max,context_pct) VALUES ('$name',$sc,$et,$mx,$pct);"
    fi
done <<< "$AGENTS"
echo "Done"
