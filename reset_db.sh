#!/bin/env bash

sqlx database drop && sqlx database create && sqlx migrate run

