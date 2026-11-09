#!/usr/bin/env bash
# Static marketing surface, shared by the Rust and frontend classifiers.
# Deliberately not all of frontend/: root layout, globals, proxy/auth, shared
# components, lib/, generated contracts, dependencies and test config affect
# the app. New routes/scripts default to the full gate until classified here.
# The (marketing) layout belongs only to the public site, not the app shell.
# Repository-wide skills and workflow checks still run for these paths.
# shellcheck disable=SC2034
MARKETING_INCLUDE='^frontend/(src/app/\(marketing\)/(page|layout)\.tsx$|src/components/marketing/|assets/marketing-social\.svg$|src/app/opengraph-image\.(png|alt\.txt)$|scripts/build-stageshow\.mjs$|tests/functional/marketing(-[a-z0-9-]+)?\.spec\.ts($|-snapshots/))'
