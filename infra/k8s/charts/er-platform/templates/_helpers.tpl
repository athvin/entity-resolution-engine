{{/* The developer name: explicit value, else the release namespace. */}}
{{- define "er-platform.developer" -}}
{{- .Values.developer | default .Release.Namespace -}}
{{- end }}

{{/* §9 Secrets Manager names: er/{env}/{namespace}/{erserver,erweb}. */}}
{{- define "er-platform.secretPath" -}}
er/{{ .Values.environment }}/{{ include "er-platform.developer" . }}
{{- end }}

{{/* Common labels (added to everything). Deliberately WITHOUT
     app.kubernetes.io/instance: that key lives in selectorLabels, and the two
     helpers are composed on pod templates — a duplicate key there breaks
     strict decoding. */}}
{{- define "er-platform.labels" -}}
app.kubernetes.io/part-of: er
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end }}

{{/* Selector labels for a component: (dict "root" . "name" "er-api"). */}}
{{- define "er-platform.selectorLabels" -}}
app.kubernetes.io/name: {{ .name }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
{{- with .color }}
er.dupezero.com/color: {{ . }}
{{- end }}
{{- end }}

{{/* The colour the Services currently select. One value, read from one place,
     so the Services and the preview Services cannot disagree about which half
     is live.

     `fail` rather than a lint, because the failure is silent otherwise and the
     chart is the only place that cannot be bypassed: a typo'd
     `--set color.active=gren` renders Services selecting a colour no Deployment
     carries, so they get no endpoints and the namespace serves nothing — while
     `helm upgrade` reports success, because every object applied cleanly. CI
     cannot catch it either; the value arrives on the deploy command line, not
     in the tree. Refusing to render is the only place this is checkable at the
     moment it is wrong. */}}
{{- define "er-platform.activeColor" -}}
{{- $active := .Values.color.active -}}
{{- if not (has $active (list "blue" "green")) -}}
{{- fail (printf "color.active must be \"blue\" or \"green\", got %q — the Services would select a colour no Deployment carries, leaving them with no endpoints while the upgrade reported success" $active) -}}
{{- end -}}
{{- $active -}}
{{- end }}

{{/* The OTHER colour: what a preview Service points at, and what the next
     deploy writes into.

     Derived rather than configured, deliberately. Two independent values would
     let a deploy set `active: blue` and `inactive: blue`, which renders a
     preview Service aimed at the live pod — and then the cutover's smoke test
     passes by testing the thing already in production, which is the one failure
     that would make the whole mechanism decorative without looking broken.
     `scripts/ci/charts-check.py` asserts the result anyway. */}}
{{- define "er-platform.inactiveColor" -}}
{{- if eq .Values.color.active "blue" -}}green{{- else -}}blue{{- end -}}
{{- end }}

{{/* Image ref for a component: (dict "root" . "image" .Values.images.api).
     Digest beats tag (§14.2 promote-by-digest); component tag beats the
     shared default. */}}
{{- define "er-platform.image" -}}
{{- $reg := .root.Values.images.registry -}}
{{- if .image.digest -}}
{{ $reg }}/{{ .image.repository }}@{{ .image.digest }}
{{- else -}}
{{ $reg }}/{{ .image.repository }}:{{ .image.tag | default .root.Values.images.tag }}
{{- end -}}
{{- end }}

{{/* The lake prefix: explicit value, else dev/{developer} (§7.3). */}}
{{- define "er-platform.lakePrefix" -}}
{{- .Values.erserver.lake.prefix | default (printf "dev/%s" (include "er-platform.developer" .)) -}}
{{- end }}

{{/* The edge hostname, when one is derivable. */}}
{{- define "er-platform.host" -}}
{{- .Values.ingress.host | default (printf "%s.dev.%s" (include "er-platform.developer" .) .Values.ingress.domain) -}}
{{- end }}

{{/* Env shared by er-api and er-dispatcher: everything non-secret the two
     erserver processes read (settings.py); secrets arrive via envFrom from
     the ExternalSecret-projected er-erserver-env. */}}
{{- define "er-platform.erserverEnv" -}}
{{/* Telemetry. RUST_LOG is the §10.4 "log level is environment-configured"
     knob, which no chart has carried until now — an empty value leaves the
     compiled default (er_obs::init::DEFAULT_FILTER), which already pins
     hyper/kube/rustls down so the first deploy with a subscriber installed
     does not bill by the gigabyte.

     OTEL_EXPORTER_OTLP_ENDPOINT is read by er-obs only when set, so shipping
     it now is what makes the exporter a no-deploy change later. */}}
{{- with .Values.otel.logLevel }}
- name: RUST_LOG
  value: {{ . | quote }}
{{- end }}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: {{ .Values.otel.endpoint | quote }}
- name: OTEL_SERVICE_NAMESPACE
  value: "er"
- name: ERSERVER_ENV
  value: {{ .Values.environment | quote }}
- name: ERSERVER_CONFIG_ROOT
  value: {{ printf "%s/configs" .Values.efs.mountPath | quote }}
- name: ERSERVER_DROP_ROOT
  value: {{ printf "%s/drop" .Values.efs.mountPath | quote }}
- name: ERSERVER_LAKE_DATA_PATH_TEMPLATE
  value: {{ printf "s3://%s/%s/{ns}/" .Values.erserver.lake.bucket (include "er-platform.lakePrefix" .) | quote }}
{{- with .Values.erserver.tenantDbPrefix }}
- name: ERSERVER_TENANT_DB_PREFIX
  value: {{ . | quote }}
{{- end }}
# The Rust control plane reads its own env names; map the deployed (Python-era)
# secret keys onto them. Per-org lake/config is carried in each org's row
# (POST /v1/orgs with ER_LAKE_* / ER_CONFIG_PATH), not the ERSERVER_* templates.
- name: ER_PG_DSN
  valueFrom:
    secretKeyRef:
      name: er-erserver-env
      key: ERSERVER_DSN
- name: ER_OPERATOR_TOKEN
  valueFrom:
    secretKeyRef:
      name: er-erserver-env
      key: ERSERVER_OPERATOR_TOKEN
{{/* Email (§7.5). Transport is the SES API, signed by the pod's EKS Pod
     Identity credentials, so there is no credential here to mount — only the
     backend selector, the verified From address and the configuration set.
     An unset backend leaves POST /v1/email answering 503, which the BFF's
     relay already treats as "not configured" and falls back from.

     The Python-era ERSERVER_EMAIL_FROM / ERSERVER_EMAIL_BASE_URL were replaced
     rather than kept beside these: nothing read either one, and BASE_URL
     implied the server builds the links, which it does not — every link
     arrives fully formed in the request's `params`. */}}
{{- with .Values.erserver.email.backend }}
- name: ER_MAIL_BACKEND
  value: {{ . | quote }}
{{- end }}
{{- with .Values.erserver.email.from }}
- name: ER_MAIL_FROM
  value: {{ . | quote }}
{{- end }}
{{- with .Values.erserver.email.configurationSet }}
- name: ER_MAIL_CONFIGURATION_SET
  value: {{ . | quote }}
{{- end }}
{{- end }}

{{/* The pod-level security context all er pods share. The namespace enforces
     PSS baseline (er-dev-namespace); this is most of restricted anyway. */}}
{{- define "er-platform.podSecurityContext" -}}
runAsNonRoot: true
seccompProfile:
  type: RuntimeDefault
{{- end }}

{{- define "er-platform.containerSecurityContext" -}}
allowPrivilegeEscalation: false
capabilities:
  drop: ["ALL"]
{{- end }}

{{/* The image for one half of one colour: (dict "root" $ "spec" $spec "half" "api").

     Each colour pins its own digests so a cutover can deploy the new image into
     the inactive colour while the live one keeps the digest it is already
     serving. That is the whole reason this is per-colour and not one
     `images.api`: a single shared value would re-render BOTH Deployments on
     every upgrade, which restarts the colour currently taking traffic — the
     outage the mechanism exists to remove.

     Falls back to `images.api`/`images.web` when a colour pins nothing, so an
     install that does not care about colours (kind, a developer namespace)
     needs no colour values at all and behaves exactly as before. */}}
{{- define "er-platform.colorImage" -}}
{{- $base := index .root.Values.images .half -}}
{{- $pin := index .spec .half -}}
{{- $image := dict
      "repository" $base.repository
      "tag" (default $base.tag $pin.tag)
      "digest" (default $base.digest $pin.digest) -}}
{{- include "er-platform.image" (dict "root" .root "image" $image) -}}
{{- end }}
