package com.example.pb;

import com.google.protobuf.GeneratedMessageLite;

/** The shape protoc's lite codegen gives message ExistenceFilter. */
public final class ExistenceFilter extends GeneratedMessageLite {
    public static final int TARGET_ID_FIELD_NUMBER = 1;
    public static final int COUNT_FIELD_NUMBER = 2;
    public static final int UNCHANGED_NAMES_FIELD_NUMBER = 3;
    private int targetId_;
    private int count_;
    private String unchangedNames_;
    public ExistenceFilter(int targetId, int count, String unchangedNames) {
        this.targetId_ = targetId;
        this.count_ = count;
        this.unchangedNames_ = unchangedNames;
    }
    @Override protected String describe() { return "E:" + targetId_ + "," + count_ + "," + unchangedNames_; }
}
