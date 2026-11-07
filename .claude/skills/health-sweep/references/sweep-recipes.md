# Sweep recipes

Copyable sequences for each phase of `/health-sweep`. Everything here is
read-only except the final section. `<env>` is the sweep argument. The
reasoning behind each read lives in the skill body; this file is the commands.

## Contents

- [Preconditions](#preconditions)
- [Alarms and their reachability](#alarms-and-their-reachability)
- [Cluster state](#cluster-state)
- [Control plane jobs](#control-plane-jobs)
- [CloudWatch logs](#cloudwatch-logs)
- [OpenObserve traces](#openobserve-traces)
- [Dedupe and file](#dedupe-and-file)

## Preconditions

```sh
aws sts get-caller-identity --profile er-readonly  # expired SSO is the usual failure
kubectl --context er-<env> get ns                  # cluster reads; absence degrades, not aborts
gh auth status                                     # scopes for filing and board reads
```

Every AWS read below runs under `AWS_PROFILE=er-readonly`.

## Alarms and their reachability

```sh
aws cloudwatch describe-alarms --state-value ALARM \
  --query 'MetricAlarms[].[AlarmName,StateReason,StateUpdatedTimestamp]' --output table
aws cloudwatch describe-alarms \
  --query 'MetricAlarms[].[AlarmName,StateValue]' --output table  # the full set, for coverage
# every topic's subscriptions — PendingConfirmation means the alarm reaches nobody
aws sns list-topics --query 'Topics[].TopicArn' --output text | tr '\t' '\n' \
  | while read -r t; do aws sns list-subscriptions-by-topic --topic-arn "$t" \
      --query 'Subscriptions[].[SubscriptionArn,Endpoint]' --output text; done
```

## Cluster state

```sh
kubectl --context er-<env> get pods -A \
  --field-selector status.phase!=Running,status.phase!=Succeeded
kubectl --context er-<env> get pods -A | awk 'NR==1 || $5 > 0'   # restart counts
kubectl --context er-<env> get events -A --field-selector type=Warning \
  --sort-by .lastTimestamp | tail -40
```

A Pending pod: the quota-versus-ResourceQuota discrimination table in
`aws-account-operations` decides which layer, before anything is concluded.
High restarts on a pod newer than its siblings: a rejected config crashlooping
the replacement while the old pod serves — the deploy that shipped it was
green.

## Control plane jobs

```sh
kubectl --context er-<env> get svc -A | grep er-api        # find the namespace
kubectl --context er-<env> -n <ns> port-forward svc/er-api 28000:8000 &
curl -fsS http://localhost:28000/healthz
# the runs/jobs read routes live in the OpenAPI document — see api-contract
```

`kubectl port-forward` exits when its stream drops; re-establish it before
diagnosing the API as down.

## CloudWatch logs

```sh
aws logs describe-log-groups --log-group-name-prefix "/er/<env>/" \
  --query 'logGroups[].logGroupName'
qid=$(aws logs start-query --log-group-name "/er/<env>/containers" \
  --start-time "$(date -v-24H +%s)" --end-time "$(date +%s)" \
  --query-string 'fields @timestamp, @logStream, @message
    | filter @message like /(?i)(error|panic|fatal)/
    | sort @timestamp desc | limit 200' --output text)
aws logs get-query-results --query-id "$qid"
```

Poll `get-query-results` until `status` is `Complete`. A result set exactly at
the `limit` is a floor, not a count — narrow the window and re-query. The
`@message` values are payload-bearing: read them here, point at them from the
issue, never paste them into it.

## OpenObserve traces

```sh
kubectl --context er-<env> -n observability port-forward svc/openobserve 25080:5080 &
# search credentials: read them from the openobserve secret in that namespace
kubectl --context er-<env> -n observability get secrets
```

Port-forward the **store**, never the collector — the collector binds its
receiver to the pod IP, so a localhost port-forward gets connection-refused
and reads as an outage. Query the search API for spans with error status in
the window; the field names (`er.outcome`, `otel.status_code`) and what a span
deliberately does not carry are the `observability` skill's subject.

## Dedupe and file

```sh
# open issues that may already cover the root cause — search by mechanism, not message
gh issue list --state open --limit 200 --search "<mechanism keywords>" \
  --json number,title,labels
# the board side, for cards filed by hand (always pass --limit)
gh project item-list 2 --owner athvin --limit 400 --format json \
  --jq '.items[] | select(.status!="Done") | "\(.content.number // "-")\t\(.title)"'

gh issue create -R athvin/entity-resolution-engine \
  --title "<the failure, not the symptom>" \
  --label followup \
  --body-file /dev/stdin <<'EOF'
**Environment:** <env>  **Window:** <UTC range>  **Found by:** /health-sweep

## What is failing
<root cause as best understood; described, never pasted>

## Evidence
- log group `<group>`, query `<the Insights query>`, <N> hits in the window
- trace <id> / run <id> / job <id>

## Blast radius
<tenant, stage or surface>

## How to reproduce the read
<the exact commands>
EOF
```

Issue bodies are public — the pointers-never-payload rule in the skill body
gates every line of that template. Auto-add puts the issue on the board; do
not `item-add`, and do not set `Status`.
