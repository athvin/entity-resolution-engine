#!/usr/bin/env bash
# Resolve the er-api / er-web image DIGESTS for a deploy (deploy.yml).
#
# §14.2: a deploy passes a DIGEST, never a tag, and gates on that digest being
# present in the registry first. Promotion-by-digest is only a property if the
# thing being deployed is identified by content.
#
# Images are tagged by full sha and built only when their own tree moved, so
# for any given commit one of them may legitimately have no sha tag. In that
# case reuse the newest digest already in the repo — that IS what is running —
# and say so. Nothing is deployed by tag either way, and an empty result is
# fatal.
#
# Inputs (env): DEPLOY_SHA, PLATFORM_ONLY ("true" on the push path, where no
# image was built by design). Must run from a checkout with full history —
# the staleness guard diffs `<built-from sha>..<DEPLOY_SHA>`, and a depth-1
# clone would make it abstain on every deploy.
# Outputs ($GITHUB_OUTPUT): er_api, er_web, reused_er_api, reused_er_web.
set -euo pipefail

for repo in er-api er-web; do
	d="$(aws ecr describe-images --repository-name "$repo" \
		--image-ids "imageTag=$DEPLOY_SHA" \
		--query 'imageDetails[0].imageDigest' --output text 2>/dev/null || true)"
	if [ -z "$d" ] || [ "$d" = "None" ]; then
		d="$(aws ecr describe-images --repository-name "$repo" \
			--query 'reverse(sort_by(imageDetails,&imagePushedAt))[0].imageDigest' \
			--output text 2>/dev/null || true)"
		# A WARNING, not just a log line. Reusing a digest is correct when
		# that tree genuinely did not change — but it also happens when the
		# build that should have produced one was skipped or never ran, and
		# then a GREEN deploy ships code from an arbitrary number of commits
		# back. Observed: the images run carrying the telemetry exporter died
		# in an outage, the next merge was frontend-only so er-api was
		# correctly skipped, and the deploy reported success while rolling an
		# image nine commits stale.
		#
		# The annotation puts it in the run summary where it is read, rather
		# than 140 lines into a step log where it was not. Two cases reach
		# here and they deserve different volumes.
		#
		# On the `workflow_run` path a missing image is SUSPICIOUS: an images
		# run did happen, so either that tree genuinely did not change or a
		# build was skipped and this deploy is about to ship older code. That
		# is the case the warning was added for, and it must stay loud.
		#
		# On the `push` path — a platform-only commit under `infra/k8s/**` —
		# no image was built BY DESIGN, so reuse is the expected and correct
		# outcome every single time. Warning there would fire on every such
		# deploy and teach the reader to ignore the annotation, which would
		# cost exactly the signal the loud case exists to provide.
		if [ "$PLATFORM_ONLY" = "true" ]; then
			echo "::notice title=$repo digest reused (platform-only deploy)::no image tagged $DEPLOY_SHA, which is expected: this commit touched only infra/k8s, so nothing was built. Rolling the newest digest in ECR ($d), which is what is already running."
		else
			echo "::warning title=$repo reused a stale digest::no image tagged $DEPLOY_SHA; rolled the newest digest in ECR instead ($d). Correct if that tree did not change in this commit — but if its build was skipped or failed, this deploy shipped older code than $DEPLOY_SHA."
		fi
		echo "$repo: no image tagged $DEPLOY_SHA — REUSED newest digest $d"
		echo "reused_${repo//-/_}=1" >> "$GITHUB_OUTPUT"

		# REFUSE, rather than warn, when the tree that builds this image
		# actually moved between what is running and what is being deployed.
		#
		# The warning above was not enough, and the evidence is specific: it
		# fired on four deploys on 2026-11-05 — P12's metrics, P15's
		# structured logging, P22's gate and P7's action header all merged,
		# green, and absent from the running binary — and nobody read it,
		# including the person who added it for exactly this failure.
		# Something that must not be missed has to fail a check, not annotate
		# one.
		#
		# The question is answerable because the reused digest carries the sha
		# it WAS built from. `git diff <that sha>..<DEPLOY_SHA>` over the
		# image's own source paths says whether this deploy would ship older
		# code than it claims. No image tag and no source change is the
		# legitimate case (#587's platform-only path, or a tree that genuinely
		# did not move); no image tag WITH a source change is the bug.
		built_from="$(aws ecr describe-images --repository-name "$repo" \
			--image-ids "imageDigest=$d" \
			--query 'imageDetails[0].imageTags[0]' --output text 2>/dev/null || true)"
		case "$repo" in
		er-api) watched="rust Dockerfile entities" ;;
		er-web) watched="frontend" ;;
		*) watched="" ;;
		esac
		if [ -n "$built_from" ] && [ "$built_from" != "None" ] && [ -n "$watched" ]; then
			# shellcheck disable=SC2086
			if ! git cat-file -e "${built_from}^{commit}" 2>/dev/null; then
				echo "::warning title=$repo cannot verify the reused digest::its tag '$built_from' is not a commit in this checkout, so the staleness check could not run"
			elif changed="$(git diff --name-only "$built_from".."$DEPLOY_SHA" -- $watched)" && [ -n "$changed" ]; then
				echo "::error title=$repo would ship older code than $DEPLOY_SHA::the newest digest in ECR was built from $built_from, and these paths changed between it and $DEPLOY_SHA. Refusing to deploy rather than reporting success on a stale image. Build it with: gh workflow run images.yml -f ref=$DEPLOY_SHA"
				echo "$changed" | head -20
				exit 1
			else
				echo "$repo: reuse verified — no change under '$watched' between $built_from and $DEPLOY_SHA"
			fi
		fi
	else
		echo "$repo: $DEPLOY_SHA -> $d"
		echo "reused_${repo//-/_}=0" >> "$GITHUB_OUTPUT"
	fi
	if [ -z "$d" ] || [ "$d" = "None" ]; then
		echo "::error::no digest in ECR for $repo; refusing to deploy by tag"
		exit 1
	fi
	echo "${repo//-/_}=$d" >> "$GITHUB_OUTPUT"
done
