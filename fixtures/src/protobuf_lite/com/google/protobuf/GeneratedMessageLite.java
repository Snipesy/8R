package com.google.protobuf;

/** Stand-in for protobuf-javalite's base class: its consumer rule keeps every subclass's fields. */
public abstract class GeneratedMessageLite {
    protected abstract String describe();

    @Override
    public String toString() {
        return describe();
    }
}
