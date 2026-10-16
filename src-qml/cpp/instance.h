// SPDX-License-Identifier: AGPL-3.0-only
#pragma once

#include <QtCore/QString>
#include <QtQml/QQmlApplicationEngine>

// One meter per data folder, so two copies never write the same settings.
// Takes the folder's lock, or, when another meter holds it, asks that meter
// to show its window and returns false. A lock left by a meter that crashed
// is taken over. Needs no QCoreApplication yet.
bool claimInstance(const QString &folder);
// Lets the meter that holds the lock hear later starts: each one calls
// showMeter() on the engine's root object. Call once Qt runs.
void listenInstance(QQmlApplicationEngine &engine);
// For tests: drops the lock and stops listening.
void releaseInstance();
// For tests: waits up to `ms` for a later start's request and reads it.
bool waitForShowRequest(int ms);
